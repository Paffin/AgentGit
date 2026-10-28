package main

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestPrivateCredentialFileOverridesOnlyTheOrchestratorToken(t *testing.T) {
	path := filepath.Join(t.TempDir(), "token")
	if err := os.WriteFile(path, []byte("private-bearer\n"), 0600); err != nil {
		t.Fatal(err)
	}
	get, err := credentialEnv(path, func(key string) string { return "previous-" + key })
	if err != nil {
		t.Fatal(err)
	}
	if get("AG_ORCH_TOKEN") != "private-bearer" || get("OTHER") != "previous-OTHER" {
		t.Fatal("credential override escaped its variable")
	}
	if runtime.GOOS != "windows" {
		if err := os.Chmod(path, 0644); err != nil {
			t.Fatal(err)
		}
		if _, err := credentialEnv(path, nil); err == nil {
			t.Fatal("publicly readable token accepted")
		}
		if err := os.Chmod(path, 0600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(path, []byte("do not reveal this secret"), 0600); err != nil {
		t.Fatal(err)
	}
	_, err = credentialEnv(path, nil)
	if err == nil || strings.Contains(err.Error(), "reveal") {
		t.Fatalf("invalid credential handling: %v", err)
	}
}
