package testutil

import (
	"os/exec"
	"path/filepath"
	"testing"
)

// BuildTestAgent compiles the TEST-ONLY agent program (./testagent) into the
// test's temporary directory and returns its path. The orchestrator runs it
// as a plain `type: command` executor. It is never part of the shipped
// binary; see the package comment of ./testagent for what it does.
func BuildTestAgent(t testing.TB) string {
	t.Helper()
	out := filepath.Join(t.TempDir(), "testagent")
	cmd := exec.Command("go", "build", "-o", out, "github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/testagent")
	if b, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("cannot build the test agent: %v\n%s", err, b)
	}
	return out
}
