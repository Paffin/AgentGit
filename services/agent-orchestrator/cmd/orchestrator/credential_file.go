package main

import (
	"fmt"
	"io"
	"os"
	"runtime"
	"strings"
)

// credentialEnv keeps the bearer token out of command arguments and config files.
func credentialEnv(path string, getenv func(string) string) (func(string) string, error) {
	token, err := readCredentialFile(path)
	if err != nil {
		return nil, err
	}
	return func(key string) string {
		if key == "AG_ORCH_TOKEN" {
			return token
		}
		if getenv != nil {
			return getenv(key)
		}
		return ""
	}, nil
}

func readCredentialFile(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("credential file: %w", err)
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return "", fmt.Errorf("credential file: %w", err)
	}
	if err := validateCredentialFile(info); err != nil {
		return "", err
	}
	raw, err := io.ReadAll(io.LimitReader(f, (16<<10)+1))
	if err != nil {
		return "", fmt.Errorf("credential file: %w", err)
	}
	token := strings.TrimSpace(string(raw))
	if token == "" || len(raw) > 16<<10 || strings.ContainsAny(token, " \t\r\n\x00") {
		return "", fmt.Errorf("credential file must contain one nonempty token")
	}
	return token, nil
}

func validateCredentialFile(info os.FileInfo) error {
	if !info.Mode().IsRegular() || info.Size() > 16<<10 {
		return fmt.Errorf("credential file must be a regular file of at most 16 KiB")
	}
	if runtime.GOOS != "windows" && info.Mode().Perm()&0077 != 0 {
		return fmt.Errorf("credential file is accessible to other users; run chmod 600 on it")
	}
	return nil
}
