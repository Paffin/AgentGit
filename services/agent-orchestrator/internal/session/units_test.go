package session

import (
	"context"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

func TestTailKeepsOnlyTheLastBytes(t *testing.T) {
	tl := newTail(10)
	tl.Write([]byte("0123456789"))
	tl.Write([]byte("abc"))
	if got := tl.Last(0); !strings.HasSuffix(got, "3456789abc") || !strings.Contains(got, "3 bytes omitted") {
		t.Fatalf("Last = %q", got)
	}
	if tl.Total() != 13 {
		t.Fatalf("total = %d", tl.Total())
	}
	big := newTail(4)
	big.Write([]byte("0123456789"))
	if !strings.HasSuffix(big.Last(0), "6789") {
		t.Fatalf("Last = %q", big.Last(0))
	}
	if got := tl.Last(3); !strings.HasSuffix(got, "abc") {
		t.Fatalf("Last(3) = %q", got)
	}
	small := newTail(100)
	small.Write([]byte("hello"))
	if small.Last(0) != "hello" {
		t.Fatalf("an untruncated tail must be returned verbatim, got %q", small.Last(0))
	}
}

func TestTailDoesNotSplitUTF8(t *testing.T) {
	tl := newTail(5)
	tl.Write([]byte("héllo wörld"))
	got := tl.Last(0)
	if !strings.HasSuffix(got, "örld") && !strings.HasSuffix(got, "wörld") && !strings.HasSuffix(got, "rld") {
		t.Fatalf("Last = %q", got)
	}
	for _, r := range got {
		if r == '�' {
			t.Fatalf("split a rune: %q", got)
		}
	}
}

func TestEnvironIsBuiltFromScratch(t *testing.T) {
	c := commandExecutor{cfg: config.Executor{Pass: []string{"PATH", "MISSING"}, Env: map[string]string{"FOO": "bar"}}}
	got := c.environ(Env{
		WorkDir: "/w", Server: "http://s", Token: secret.New("tok"), RunID: "run_1", Repo: "a/b", Stream: "main",
		Task: agwp.Task{ID: "task_1"}, Agent: "bot", Identity: "agent:bot#run_1", KeyFile: "/k", ResultFile: "/r", TaskFile: "/t",
		ParentEnv: func(k string) string { return map[string]string{"PATH": "/bin", "SECRET": "nope"}[k] },
	})
	want := []string{
		"AG_AGENT=bot", "AG_IDENTITY=agent:bot#run_1", "AG_KEY_FILE=/k", "AG_REPO=a/b", "AG_RESULT_FILE=/r", "AG_RUN=run_1",
		"AG_SERVER=http://s", "AG_STREAM=main", "AG_TASK=task_1", "AG_TASK_FILE=/t", "AG_TOKEN=tok", "AG_WORKDIR=/w",
		"FOO=bar", "HOME=/w", "PATH=/bin",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("environ:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestSummaryHelpers(t *testing.T) {
	if got := oneLine("a\nb   c"); got != "a | b c" {
		t.Fatalf("oneLine = %q", got)
	}
	long := strings.Repeat("x", 800)
	if got := oneLine(long); len(got) > 504 || !strings.HasSuffix(got, "...") {
		t.Fatalf("oneLine length %d", len(got))
	}
	if got := tailOneLine("first\nsecond\nthird", 8); got != "...| third" && got != "...third" && !strings.HasSuffix(got, "third") {
		t.Fatalf("tailOneLine = %q", got)
	}
	if signalName(9) != "SIGKILL" || signalName(15) != "SIGTERM" || signalName(31) != "signal 31" {
		t.Fatal("signalName")
	}
	if Keep.String() != "keep" || Done.String() != "done" || Cancel.String() != "cancel" {
		t.Fatal("Disposition.String")
	}
	if !(Exit{}).OK() || (Exit{Code: 1}).OK() || (Exit{Signal: "SIGKILL", Code: -1}).OK() || (Exit{Err: context.Canceled}).OK() {
		t.Fatal("Exit.OK")
	}
}

// ChargeOf reads the summaries result() writes; the two must not drift apart.
func TestChargeOfMatchesTheSummariesTheSessionWrites(t *testing.T) {
	for _, tt := range []struct {
		name   string
		status string
		sum    string
		want   Charge
	}{
		{"exit code", "failed", "failed: executor bot exited with code 3 (attempt 2)", ChargeAttempt},
		{"temporary failure", "failed", "failed: executor bot exited with code 75 (attempt 2)", ChargeTemporary},
		{"killed", "failed", "abandoned: the executor was killed by signal SIGKILL (attempt 2)", ChargeAttempt},
		{"timeout", "failed", "timeout: the session exceeded 30m; the executor was stopped (attempt 2)", ChargeAttempt},
		{"cancelled", "failed", "cancelled: orchestrator shutdown", ChargeNone},
		{"refused before it ran", "failed", "deferred: lease conflict", ChargeNone},
		{"lost with its orchestrator", "failed", LostRunPrefix + " for 10m0s and no session of this orchestrator supervises the run", ChargeNone},
		{"completed", "completed", "completed by executor bot (attempt 2, 1s)", ChargeNone},
	} {
		if got := ChargeOf(tt.status, tt.sum); got != tt.want {
			t.Errorf("%s: ChargeOf(%q, %q) = %v, want %v", tt.name, tt.status, tt.sum, got, tt.want)
		}
	}
}
