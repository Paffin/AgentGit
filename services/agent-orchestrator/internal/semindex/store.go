package semindex

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Store persists an index as a directory of small JSON files: one head.json
// and one file per indexed source file, named by the hash of its path. No
// database, no dependency; a damaged file is simply re-indexed.
type Store struct{ dir string }

type headFile struct {
	Version int    `json:"version"`
	Repo    string `json:"repo"`
	Head    string `json:"head"`
}

const storeVersion = 1

// OpenStore creates the repository's index directory below root.
func OpenStore(root, repo string) (*Store, error) {
	if root == "" {
		return nil, errors.New("semindex: index directory not configured")
	}
	dir := filepath.Join(root, strings.ReplaceAll(repo, "/", "__"))
	if err := os.MkdirAll(filepath.Join(dir, "files"), 0o700); err != nil {
		return nil, err
	}
	return &Store{dir: dir}, nil
}

func (s *Store) entryPath(p string) string {
	sum := sha256.Sum256([]byte(p))
	h := hex.EncodeToString(sum[:])
	return filepath.Join(s.dir, "files", h[:2], h[2:34]+".json")
}

// writeAtomic writes through a temporary file so a crash never leaves a torn file.
func writeAtomic(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".tmp-*")
	if err != nil {
		return err
	}
	name := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(name)
		return err
	}
	if err := tmp.Chmod(0o600); err != nil {
		tmp.Close()
		os.Remove(name)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(name)
		return err
	}
	return os.Rename(name, path)
}

// SaveEntry stores one file's symbols.
func (s *Store) SaveEntry(e *FileEntry) error {
	b, err := json.Marshal(e)
	if err != nil {
		return err
	}
	return writeAtomic(s.entryPath(e.Path), b)
}

// DeleteEntry removes one file's symbols.
func (s *Store) DeleteEntry(path string) error {
	err := os.Remove(s.entryPath(path))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// SaveHead records the stream head the stored entries describe.
func (s *Store) SaveHead(repo, head string) error {
	b, _ := json.Marshal(headFile{Version: storeVersion, Repo: repo, Head: head})
	return writeAtomic(filepath.Join(s.dir, "head.json"), b)
}

// Load reads the stored index. Unreadable entries are skipped and counted; the
// next sync fetches them again because their blob will not match.
func (s *Store) Load(repo string, maxFiles int) (*Index, int, error) {
	x := NewIndex(repo)
	bad := 0
	var hf headFile
	if b, err := os.ReadFile(filepath.Join(s.dir, "head.json")); err == nil {
		if json.Unmarshal(b, &hf) != nil || hf.Version != storeVersion || hf.Repo != repo {
			return x, 0, nil // another format or repository: start fresh
		}
	} else if errors.Is(err, os.ErrNotExist) {
		return x, 0, nil
	} else {
		return nil, 0, err
	}
	var entries []*FileEntry
	err := filepath.WalkDir(filepath.Join(s.dir, "files"), func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(p, ".json") {
			return nil
		}
		if len(entries) >= maxFiles {
			bad++
			return nil
		}
		b, rerr := os.ReadFile(p)
		var e FileEntry
		if rerr != nil || json.Unmarshal(b, &e) != nil || e.Path == "" || s.entryPath(e.Path) != p {
			bad++
			return nil
		}
		entries = append(entries, &e)
		return nil
	})
	if err != nil {
		return nil, 0, fmt.Errorf("semindex: reading %s: %w", s.dir, err)
	}
	x.apply(entries, nil, hf.Head, Skipped{}, nil)
	return x, bad, nil
}
