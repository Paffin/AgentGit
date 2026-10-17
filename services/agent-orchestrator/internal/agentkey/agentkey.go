// Package agentkey stores Ed25519 agent keys in the same file format the
// AgentGit CLI uses (version 1, hex seed), so a key generated here can be used
// by "ag" and the other way round.
//
// The private half of an agent key is what lets someone speak as that agent
// when signing provenance. The orchestrator hands the path of the key file to
// the agent's executor (AG_KEY_FILE) and never an orchestrator credential.
package agentkey

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

var nameRE = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,63}$`)

// ValidName reports whether name is a valid agent name (server rule).
func ValidName(name string) bool { return nameRE.MatchString(name) }

type keyFile struct {
	Version   int    `json:"version"`
	Algorithm string `json:"algorithm"`
	Secret    string `json:"secret"`
	Public    string `json:"public"`
}

// Key is an Ed25519 key pair.
type Key struct {
	priv ed25519.PrivateKey
}

// Generate makes a fresh key.
func Generate() (*Key, error) {
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	return &Key{priv: priv}, nil
}

// FromSeed derives a key from a 32-byte seed (tests).
func FromSeed(seed []byte) (*Key, error) {
	if len(seed) != ed25519.SeedSize {
		return nil, fmt.Errorf("agentkey: seed must be %d bytes", ed25519.SeedSize)
	}
	return &Key{priv: ed25519.NewKeyFromSeed(seed)}, nil
}

// PublicHex is the 64-hex-character public key the server registers.
func (k *Key) PublicHex() string {
	return hex.EncodeToString(k.priv.Public().(ed25519.PublicKey))
}

// Sign signs payload under a domain (docs/protocol/agwp-v1.md#signing-formats)
// and returns the 128-hex-character signature.
func (k *Key) Sign(domain string, payload []byte) (string, error) {
	if domain == "" || len(domain) > 64 {
		return "", errors.New("agentkey: signature domain must be 1..64 bytes")
	}
	return hex.EncodeToString(ed25519.Sign(k.priv, agwp.SignedMessage(domain, payload))), nil
}

// Verify checks a hex signature made by Sign.
func Verify(publicHex, domain string, payload []byte, sigHex string) bool {
	pub, err := hex.DecodeString(publicHex)
	if err != nil || len(pub) != ed25519.PublicKeySize {
		return false
	}
	sig, err := hex.DecodeString(sigHex)
	if err != nil || len(sig) != ed25519.SignatureSize {
		return false
	}
	return ed25519.Verify(pub, agwp.SignedMessage(domain, payload), sig)
}

// Save writes the key to path with mode 0600, refusing to overwrite.
func (k *Key) Save(path string) error {
	b, err := json.MarshalIndent(keyFile{
		Version: 1, Algorithm: "ed25519",
		Secret: hex.EncodeToString(k.priv.Seed()), Public: k.PublicHex(),
	}, "", "  ")
	if err != nil {
		return err
	}
	// Write a private temporary file completely, then publish it under its final
	// name with a hard link: the link is atomic and fails if the name exists, so
	// a concurrent reader never sees a half-written key and two writers never
	// both win.
	tmp, err := os.CreateTemp(filepath.Dir(path), ".key-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if err := tmp.Chmod(0o600); err != nil {
		tmp.Close()
		return err
	}
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Link(tmp.Name(), path)
}

// Load reads a key file, refusing files readable by group or others.
func Load(path string) (*Key, error) {
	st, err := os.Stat(path)
	if err != nil {
		return nil, err
	}
	if st.Mode().Perm()&0o077 != 0 {
		return nil, fmt.Errorf("agentkey: %s is accessible to group or others (mode %o); chmod 600 it", path, st.Mode().Perm())
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var kf keyFile
	if err := json.Unmarshal(b, &kf); err != nil {
		return nil, fmt.Errorf("agentkey: %s: %w", path, err)
	}
	if kf.Version != 1 || kf.Algorithm != "ed25519" {
		return nil, fmt.Errorf("agentkey: %s: unsupported version or algorithm", path)
	}
	seed, err := hex.DecodeString(kf.Secret)
	if err != nil {
		return nil, fmt.Errorf("agentkey: %s: bad secret", path)
	}
	k, err := FromSeed(seed)
	if err != nil {
		return nil, fmt.Errorf("agentkey: %s: %w", path, err)
	}
	if k.PublicHex() != kf.Public {
		return nil, fmt.Errorf("agentkey: %s: public key does not match the secret", path)
	}
	return k, nil
}

// Store is a directory of agent keys, one <name>.json each.
type Store struct{ dir string }

// OpenStore creates (if needed) and returns a key directory with mode 0700.
func OpenStore(dir string) (*Store, error) {
	if dir == "" {
		return nil, errors.New("agentkey: key directory not configured")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		return nil, err
	}
	return &Store{dir: dir}, nil
}

// Path returns the file for an agent's key.
func (s *Store) Path(name string) string { return filepath.Join(s.dir, name+".json") }

// Ensure loads the agent's key, generating and saving it on first use.
func (s *Store) Ensure(name string) (*Key, error) {
	if !ValidName(name) {
		return nil, fmt.Errorf("agentkey: invalid agent name %q (want [a-z0-9][a-z0-9_-]{0,63})", name)
	}
	p := s.Path(name)
	k, err := Load(p)
	if err == nil {
		return k, nil
	}
	if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	k, err = Generate()
	if err != nil {
		return nil, err
	}
	if err := k.Save(p); err != nil {
		if errors.Is(err, os.ErrExist) { // lost a race with another process
			return Load(p)
		}
		return nil, err
	}
	return k, nil
}
