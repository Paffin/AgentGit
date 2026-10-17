package session

import (
	"context"
	"strings"
	"testing"
	"time"

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

func TestParseTaskScript(t *testing.T) {
	desc := "Please do the thing.\nag-script: {\"files\": {\"a.txt\": \"A\\n\"}, \"delete\": [\"old.txt\"], \"title\": \"T\", \"message\": \"M\"}\nthanks"
	ts, ok := parseTaskScript(desc)
	if !ok || ts.Files["a.txt"] != "A\n" || len(ts.Delete) != 1 || ts.Title != "T" || ts.Message != "M" {
		t.Fatalf("ts = %+v ok=%v", ts, ok)
	}
	for _, bad := range []string{"", "no marker here", "ag-script: not json", "ag-script: {\"unknown\": 1}"} {
		if _, ok := parseTaskScript(bad); ok {
			t.Errorf("%q must not parse", bad)
		}
	}
}

func TestTaskDescriptionsOnlyDriveTheScriptedExecutorWhenEnabled(t *testing.T) {
	e := newEnv(t)
	e.fake.SetHead(repo, "main", e.fake.Commit(repo, nil, map[string]string{"old.txt": "old"}, "user:root", "seed"))
	script := `ag-script: {"files": {"new.txt": "from the task"}, "delete": ["old.txt"], "title": "From task"}`
	mkTask := func() agwp.Task {
		id := e.fake.AddTask(repo, "scripted task", []string{"**"})
		e.fake.SetTaskDescription(repo, id, script)
		return e.fake.Task(repo, id)
	}

	// Disabled: the description is untrusted text and is ignored; with no files configured nothing happens.
	off := scriptedExec("quiet-bot", nil)
	off.Script.TaskFiles = false
	if _, err := e.mgr.Start(context.Background(), e.request(mkTask(), off, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || len(res.Proposals) != 0 || !strings.Contains(res.Summary, "nothing to do") {
		t.Fatalf("task descriptions must be ignored by default: %+v", res)
	}

	on := scriptedExec("script-bot", nil)
	on.Script.TaskFiles = true
	if _, err := e.mgr.Start(context.Background(), e.request(mkTask(), on, Keep)); err != nil {
		t.Fatal(err)
	}
	res = e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCompleted || len(res.Proposals) != 1 || res.Proposals[0].Title != "From task" {
		t.Fatalf("result = %+v", res)
	}
	head := res.Proposals[0].Head
	if b, ok := e.fake.File(repo, head, "new.txt"); !ok || string(b) != "from the task" {
		t.Fatalf("new.txt = %q %v", b, ok)
	}
	if _, ok := e.fake.File(repo, head, "old.txt"); ok {
		t.Fatal("old.txt should be deleted")
	}
}

func TestScriptedExecutorReportsServerRefusals(t *testing.T) {
	e := newEnv(t)
	e.fake.SetHead(repo, "main", e.fake.Commit(repo, nil, map[string]string{"a": "b"}, "user:root", "seed"))
	// An agent credential without workspace.write cannot even start.
	ex := scriptedExec("weak-bot", map[string]string{"x": "y"}, func(c *config.Executor) {
		c.Ops = []string{"repo.read", "task.read", "agent.run", "lease.read", "lease.write"}
	})
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || !strings.Contains(res.Summary, "create workspace") || !strings.Contains(res.Summary, "permission_denied") {
		t.Fatalf("result = %+v", res)
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
