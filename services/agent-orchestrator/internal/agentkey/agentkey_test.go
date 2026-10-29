package agentkey

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// RFC 8032, test vector 1.
const (
	rfcSeed   = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
	rfcPublic = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)

func TestFromSeedMatchesTheReferenceVector(t *testing.T) {
	seed, _ := hex.DecodeString(rfcSeed)
	k, err := FromSeed(seed)
	if err != nil {
		t.Fatal(err)
	}
	if k.PublicHex() != rfcPublic {
		t.Fatalf("public key = %s", k.PublicHex())
	}
	if _, err := FromSeed([]byte("short")); err == nil {
		t.Fatal("a short seed must be refused")
	}
}

func TestSignAndVerifyAreDomainSeparated(t *testing.T) {
	k, _ := Generate()
	sig, err := k.Sign("provenance.v1", []byte("payload"))
	if err != nil || len(sig) != 128 {
		t.Fatalf("sig=%q err=%v", sig, err)
	}
	if !Verify(k.PublicHex(), "provenance.v1", []byte("payload"), sig) {
		t.Fatal("valid signature rejected")
	}
	for name, ok := range map[string]bool{
		"other domain":  Verify(k.PublicHex(), "attestation.v1", []byte("payload"), sig),
		"other payload": Verify(k.PublicHex(), "provenance.v1", []byte("payload2"), sig),
		"bad hex":       Verify(k.PublicHex(), "provenance.v1", []byte("payload"), "zz"),
		"short sig":     Verify(k.PublicHex(), "provenance.v1", []byte("payload"), sig[:10]),
		"bad key":       Verify("00", "provenance.v1", []byte("payload"), sig),
	} {
		if ok {
			t.Errorf("%s: verification must fail", name)
		}
	}
	other, _ := Generate()
	if Verify(other.PublicHex(), "provenance.v1", []byte("payload"), sig) {
		t.Fatal("another key verified the signature")
	}
	if _, err := k.Sign("", nil); err == nil {
		t.Fatal("an empty domain must be refused")
	}
	if _, err := k.Sign(strings.Repeat("d", 65), nil); err == nil {
		t.Fatal("a domain over 64 bytes must be refused")
	}
}

func TestSaveWritesTheCLIKeyFileFormatWith0600(t *testing.T) {
	k, _ := Generate()
	p := filepath.Join(t.TempDir(), "k.json")
	if err := k.Save(p); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(p)
	if st.Mode().Perm() != 0o600 {
		t.Fatalf("mode = %v", st.Mode().Perm())
	}
	var kf map[string]any
	b, _ := os.ReadFile(p)
	if err := json.Unmarshal(b, &kf); err != nil {
		t.Fatal(err)
	}
	if kf["version"] != float64(1) || kf["algorithm"] != "ed25519" || kf["public"] != k.PublicHex() || len(kf["secret"].(string)) != 64 {
		t.Fatalf("key file = %v", kf)
	}
	if err := k.Save(p); err == nil {
		t.Fatal("Save must not overwrite an existing key")
	}
	back, err := Load(p)
	if err != nil || back.PublicHex() != k.PublicHex() {
		t.Fatalf("load: %v", err)
	}
}

func TestLoadRefusesWorldReadableAndCorruptKeys(t *testing.T) {
	k, _ := Generate()
	dir := t.TempDir()
	p := filepath.Join(dir, "k.json")
	_ = k.Save(p)
	if err := os.Chmod(p, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(p); err == nil || !strings.Contains(err.Error(), "accessible to group or others") {
		t.Fatalf("err = %v", err)
	}
	_ = os.Chmod(p, 0o600)
	for name, content := range map[string]string{
		"not json":        "nope",
		"wrong version":   `{"version":2,"algorithm":"ed25519","secret":"` + rfcSeed + `","public":"` + rfcPublic + `"}`,
		"wrong algorithm": `{"version":1,"algorithm":"rsa","secret":"` + rfcSeed + `","public":"` + rfcPublic + `"}`,
		"bad secret":      `{"version":1,"algorithm":"ed25519","secret":"zz","public":"` + rfcPublic + `"}`,
		"mismatch":        `{"version":1,"algorithm":"ed25519","secret":"` + rfcSeed + `","public":"` + strings.Repeat("0", 64) + `"}`,
	} {
		bad := filepath.Join(dir, strings.ReplaceAll(name, " ", "_")+".json")
		if err := os.WriteFile(bad, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := Load(bad); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	if _, err := Load(filepath.Join(dir, "missing.json")); !os.IsNotExist(err) {
		t.Errorf("missing file: %v", err)
	}
}

func TestStoreCreatesOnceAndReusesTheKey(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "keys")
	s, err := OpenStore(dir)
	if err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(dir)
	if st.Mode().Perm() != 0o700 {
		t.Fatalf("key directory mode = %v", st.Mode().Perm())
	}
	a, err := s.Ensure("docs-bot")
	if err != nil {
		t.Fatal(err)
	}
	b, err := s.Ensure("docs-bot")
	if err != nil || a.PublicHex() != b.PublicHex() {
		t.Fatalf("the key changed between calls: %v", err)
	}
	if s.Path("docs-bot") != filepath.Join(dir, "docs-bot.json") {
		t.Fatal(s.Path("docs-bot"))
	}
}

func TestStoreIsSafeUnderConcurrentFirstUse(t *testing.T) {
	s, _ := OpenStore(t.TempDir())
	var wg sync.WaitGroup
	keys := make([]string, 8)
	for i := range keys {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			k, err := s.Ensure("racy-bot")
			if err != nil {
				t.Error(err)
				return
			}
			keys[i] = k.PublicHex()
		}(i)
	}
	wg.Wait()
	for _, k := range keys {
		if k != keys[0] {
			t.Fatalf("concurrent callers saw different keys: %v", keys)
		}
	}
}

func TestStoreRejectsBadNames(t *testing.T) {
	s, _ := OpenStore(t.TempDir())
	for _, bad := range []string{"", "Upper", "has space", "../escape", "a/b", "-leading", strings.Repeat("a", 65)} {
		if _, err := s.Ensure(bad); err == nil {
			t.Errorf("name %q accepted", bad)
		}
		if ValidName(bad) {
			t.Errorf("ValidName(%q)", bad)
		}
	}
	for _, ok := range []string{"a", "docs-bot", "bot_2", "0day", strings.Repeat("a", 64)} {
		if !ValidName(ok) {
			t.Errorf("ValidName(%q) = false", ok)
		}
	}
	if _, err := OpenStore(""); err == nil {
		t.Fatal("an empty directory must be refused")
	}
}

func TestPendingPrivateKeyPrefixIsWithheld(t *testing.T) {
	seed, _ := hex.DecodeString(rfcSeed)
	key, err := FromSeed(seed)
	if err != nil {
		t.Fatal(err)
	}
	if got := key.PendingSuffixLength("diagnostic\n" + rfcSeed[:24]); got != 24 {
		t.Fatalf("pending private seed prefix length = %d", got)
	}
	if got := key.PendingSuffixLength("ordinary output\n"); got != 0 {
		t.Fatalf("ordinary output withheld: %d", got)
	}
}
