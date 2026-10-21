package testutil

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestListRequires(t *testing.T) {
	for _, c := range []struct {
		list, component string
		want            bool
	}{
		{"postgres,s3", "s3", true},
		{" postgres , s3 ", "postgres", true},
		{"postgres s3", "s3", true},
		{"postgres,s3", "keycloak", false},
		{"", "postgres", false},
		{"postgres", "post", false},
		{"ALL", "anything", true},
		{"oci", "oci:docker", true},
		{"oci:docker", "oci", true},
		{"oci:docker", "oci:podman", false},
	} {
		if got := ListRequires(c.list, c.component); got != c.want {
			t.Errorf("ListRequires(%q, %q) = %v, want %v", c.list, c.component, got, c.want)
		}
	}
}

func TestRequiredFollowsTheEnvironment(t *testing.T) {
	t.Setenv("CI", "")
	t.Setenv("AG_REQUIRE_SERVER", "")
	t.Setenv(envRequire, "")
	if Required("model") || Required("ag-server-binary") {
		t.Fatal("nothing is required by default")
	}
	t.Setenv(envRequire, "ag-server-binary")
	if !Required("ag-server-binary") || Required("model") {
		t.Fatal("only the named component is required")
	}
	t.Setenv(envRequire, "")
	t.Setenv("CI", "true")
	if !Required("ag-server-binary") {
		t.Fatal("CI has always meant: the real-server tests must not skip")
	}
	if Required("model") {
		t.Fatal("CI does not require a model")
	}
}

func TestASkipLeavesAMarkerInTheReport(t *testing.T) {
	report := filepath.Join(t.TempDir(), "skips.jsonl")
	t.Setenv("AG_SKIP_REPORT", report)
	t.Setenv(envRequire, "")
	t.Setenv("CI", "")
	t.Run("inner", func(t *testing.T) {
		RequireComponent(t, "model", errors.New("AG_LLM_LIVE is\nnot set"))
		t.Fatal("RequireComponent must have skipped")
	})
	data, err := os.ReadFile(report)
	if err != nil {
		t.Fatal(err)
	}
	var got map[string]string
	if err := json.Unmarshal([]byte(strings.TrimSpace(string(data))), &got); err != nil {
		t.Fatalf("%v: %q", err, data)
	}
	if got["marker"] != "AG-SKIP" || got["component"] != "model" || got["test"] != t.Name()+"/inner" || got["reason"] != "AG_LLM_LIVE is not set" || got["lang"] != "go" {
		t.Fatalf("%v", got)
	}
}

func TestAnAvailableComponentDoesNothing(t *testing.T) {
	report := filepath.Join(t.TempDir(), "skips.jsonl")
	t.Setenv("AG_SKIP_REPORT", report)
	RequireComponent(t, "model", nil)
	if _, err := os.Stat(report); err == nil {
		t.Fatal("no marker for a component that is there")
	}
}

func TestTheMarkerIsOneLine(t *testing.T) {
	if got := Marker("model", "T", "a\nb   c"); got != "AG-SKIP component=model test=T reason=a b c" {
		t.Fatal(got)
	}
}
