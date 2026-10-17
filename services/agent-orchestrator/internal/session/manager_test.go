package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

type env struct {
	t       *testing.T
	fake    *fakeagwp.Server
	orch    *agwp.Client
	orchTok string
	mgr     *Manager
	keys    *agentkey.Store
	metrics *metrics.Service
	dir     string
	results chan Result
}

func newEnv(t *testing.T) *env {
	t.Helper()
	fake := fakeagwp.New()
	t.Cleanup(fake.Close)
	fake.AddRepo(repo)
	e := &env{t: t, fake: fake, dir: t.TempDir(), results: make(chan Result, 16), metrics: metrics.NewService(metrics.NewRegistry())}
	e.orchTok = fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin"})
	var err error
	e.orch, err = agwp.New(fake.URL, secret.New(e.orchTok), agwp.WithIdentityLabel("orchestrator"), agwp.WithObserver(e.metrics))
	if err != nil {
		t.Fatal(err)
	}
	e.keys, err = agentkey.OpenStore(filepath.Join(e.dir, "keys"))
	if err != nil {
		t.Fatal(err)
	}
	e.mgr, err = NewManager(Options{
		Orch: e.orch, ServerURL: fake.URL, Keys: e.keys, WorkRoot: filepath.Join(e.dir, "work"), Metrics: e.metrics,
		Log:       slog.New(slog.NewTextHandler(io.Discard, nil)),
		ParentEnv: func(k string) string { return map[string]string{"PATH": os.Getenv("PATH"), "LEAKY": "parent-value"}[k] },
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.mgr.Close)
	return e
}

// Observe adapts metrics for the client observer in tests.
func (e *env) request(task agwp.Task, ex config.Executor, decide Disposition) Request {
	return Request{
		Repo: repo, Stream: "main", Task: task, Executor: ex, Attempt: 1, Lease: "soft", LeaseTTL: 5 * time.Minute,
		Decide: func(r Result) Disposition { e.results <- r; return decide },
	}
}

func (e *env) task(title string, scope ...string) agwp.Task {
	id := e.fake.AddTask(repo, title, scope)
	return e.fake.Task(repo, id)
}

func (e *env) wait(d time.Duration) Result {
	e.t.Helper()
	select {
	case r := <-e.results:
		return r
	case <-time.After(d):
		e.t.Fatal("the session did not end in time")
		return Result{}
	}
}

func (e *env) idle() {
	e.t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for len(e.mgr.Active()) != 0 {
		if time.Now().After(deadline) {
			e.t.Fatalf("sessions still active: %+v", e.mgr.Active())
		}
		time.Sleep(2 * time.Millisecond)
	}
}

func commandExec(t *testing.T, name, script string, mut ...func(*config.Executor)) config.Executor {
	t.Helper()
	path := filepath.Join(t.TempDir(), name+".sh")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+script), 0o700); err != nil {
		t.Fatal(err)
	}
	ex := config.Executor{Name: name, Type: "command", Agent: name, Slots: 2, Timeout: 20 * time.Second, Grace: 500 * time.Millisecond,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Command: []string{"/bin/sh", path}, Pass: []string{"PATH"}, Env: map[string]string{}}
	for _, f := range mut {
		f(&ex)
	}
	return ex
}

func scriptedExec(name string, files map[string]string, mut ...func(*config.Executor)) config.Executor {
	ex := config.Executor{Name: name, Type: "scripted", Agent: name, Slots: 2, Timeout: 20 * time.Second, Grace: time.Second,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Script: config.Scripted{Files: files, Message: "scripted change", Title: "Scripted change", Model: "demo-model", Rationale: "because"}}
	for _, f := range mut {
		f(&ex)
	}
	return ex
}

func TestScriptedSessionProposesSignsAndCompletes(t *testing.T) {
	e := newEnv(t)
	e.fake.SetHead(repo, "main", e.fake.Commit(repo, nil, map[string]string{"README.md": "hi\n"}, "user:root", "seed"))
	task := e.task("Add docs", "docs/**")
	info, err := e.mgr.Start(context.Background(), e.request(task, scriptedExec("docs-bot", map[string]string{"docs/guide.md": "# Guide\n"}), Keep))
	if err != nil {
		t.Fatal(err)
	}
	if info.State != "running" || info.Agent != "docs-bot" || len(info.Leases) != 1 || info.Leases[0].Scope != "docs/**" || info.Leases[0].Exclusive {
		t.Fatalf("info = %+v", info)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCompleted || len(res.Proposals) != 1 {
		t.Fatalf("result = %+v", res)
	}
	p := res.Proposals[0]
	if p.Author != "agent:docs-bot#"+res.RunID || p.TaskID != task.ID || p.State != agwp.ProposalOpen {
		t.Fatalf("proposal = %+v", p)
	}
	// provenance was submitted by the agent for exactly the proposed head, and the file is in the workspace head
	prov := e.fake.Provenance(repo)
	if len(prov) != 1 || prov[0].Record.Changeset != p.Head || prov[0].Record.TaskID != task.ID || prov[0].Record.Agent != p.Author || len(prov[0].Signature) != 128 {
		t.Fatalf("provenance = %+v", prov)
	}
	if b, ok := e.fake.File(repo, p.Head, "docs/guide.md"); !ok || string(b) != "# Guide\n" {
		t.Fatalf("file = %q %v", b, ok)
	}
	// run completed, lease released by the run's completion, credential revoked, task left in progress
	runs := e.fake.Runs(repo)
	if len(runs) != 1 || runs[0].Status != "completed" || !strings.Contains(runs[0].Summary, p.ID) {
		t.Fatalf("runs = %+v", runs)
	}
	for _, l := range e.fake.Leases(repo) {
		if l.ReleasedAtMs == 0 {
			t.Fatalf("lease %+v still held", l)
		}
	}
	for _, m := range e.fake.MintedTokens() {
		if !m.Revoked {
			t.Fatalf("credential %+v was not revoked", m)
		}
	}
	if got := e.fake.Task(repo, task.ID); got.State != agwp.TaskInProgress || got.Owner != "agent:docs-bot" {
		t.Fatalf("task = %+v", got)
	}
	if e.metrics.SessionsStarted.With(repo, "docs-bot").Value() != 1 || e.metrics.SessionsFinished.With(repo, "docs-bot", "completed").Value() != 1 {
		t.Fatal("session metrics not recorded")
	}
	if e.metrics.SessionsRunning.With(repo, "docs-bot").Value() != 0 {
		t.Fatal("running gauge not back to zero")
	}
}

func TestAgentCredentialIsNarrowAndShortLived(t *testing.T) {
	e := newEnv(t)
	e.fake.SetHead(repo, "main", e.fake.Commit(repo, nil, map[string]string{"a": "b"}, "user:root", "seed"))
	ex := scriptedExec("scoped-bot", map[string]string{"x.txt": "x"}, func(c *config.Executor) { c.Cred = 7 * time.Minute })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(10 * time.Second)
	e.idle()
	minted := e.fake.MintedTokens()
	if len(minted) != 1 {
		t.Fatalf("minted = %+v", minted)
	}
	m := minted[0]
	if m.Identity != "agent:scoped-bot" || len(m.Repos) != 1 || m.Repos[0] != repo || len(m.Streams) != 1 || m.Streams[0] != "main" {
		t.Fatalf("scope = %+v", m)
	}
	for _, op := range m.Ops {
		if config.ForbiddenAgentOps[op] {
			t.Fatalf("the agent credential carries %q", op)
		}
	}
	if ttl := time.Until(m.ExpiresAt); ttl <= 0 || ttl > 7*time.Minute+5*time.Second {
		t.Fatalf("ttl = %v", ttl)
	}
}

func TestCommandExecutorGetsExactlyTheDocumentedEnvironment(t *testing.T) {
	e := newEnv(t)
	dump := filepath.Join(t.TempDir(), "env.txt")
	ex := commandExec(t, "env-bot", `env | sort > "$DUMP"; cat "$AG_TASK_FILE" > "$DUMP.task"; ls -l "$AG_KEY_FILE" | cut -c1-10 > "$DUMP.keymode"; echo '{"summary":"dumped the environment"}' > "$AG_RESULT_FILE"`,
		func(c *config.Executor) {
			c.Env = map[string]string{"DUMP": dump}
			c.Pass = []string{"PATH", "LEAKY_NOT_LISTED"}
		})
	task := e.task("env check", "src/**")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Done)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCompleted || !strings.Contains(res.Summary, "dumped the environment") {
		t.Fatalf("result = %+v", res)
	}
	raw, err := os.ReadFile(dump)
	if err != nil {
		t.Fatal(err)
	}
	vars := map[string]string{}
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		k, v, _ := strings.Cut(line, "=")
		vars[k] = v
	}
	for k, want := range map[string]string{
		"AG_SERVER": e.fake.URL, "AG_RUN": res.RunID, "AG_REPO": repo, "AG_STREAM": "main", "AG_TASK": task.ID,
		"AG_AGENT": "env-bot", "AG_IDENTITY": "agent:env-bot#" + res.RunID, "DUMP": dump,
	} {
		if vars[k] != want {
			t.Errorf("%s = %q, want %q", k, vars[k], want)
		}
	}
	for _, k := range []string{"AG_TOKEN", "AG_KEY_FILE", "AG_WORKDIR", "AG_RESULT_FILE", "AG_TASK_FILE"} {
		if vars[k] == "" {
			t.Errorf("%s is missing", k)
		}
	}
	if vars["HOME"] != vars["AG_WORKDIR"] {
		t.Errorf("HOME = %q, want the session work directory", vars["HOME"])
	}
	if _, leaked := vars["LEAKY"]; leaked {
		t.Error("an environment variable that was not passed through reached the executor")
	}
	// The token the executor got is the agent's, not the orchestrator's.
	if strings.Contains(string(raw), e.orchTok) {
		t.Fatal("the orchestrator's own credential reached the executor")
	}
	minted := e.fake.MintedTokens()
	if len(minted) != 1 || vars["AG_TOKEN"] != minted[0].Secret {
		t.Fatalf("AG_TOKEN is not the minted session credential")
	}
	if mode, _ := os.ReadFile(dump + ".keymode"); !strings.HasPrefix(string(mode), "-rw-------") {
		t.Errorf("key file mode = %q", mode)
	}
	if task, _ := os.ReadFile(dump + ".task"); !strings.Contains(string(task), "env check") {
		t.Errorf("task file = %s", task)
	}
	// Done disposition: the owner (the agent) finished the task.
	if got := e.fake.Task(repo, task.ID); got.State != agwp.TaskDone {
		t.Fatalf("task = %+v", got)
	}
	if _, err := os.Stat(filepath.Join(e.dir, "work", res.RunID)); !os.IsNotExist(err) {
		t.Fatal("the session work directory was not removed")
	}
}

func TestFailingExecutorFailsTheRunAndCancelsTheTask(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "bad-bot", `echo "working..."; echo "boom: could not compile" >&2; exit 3`)
	task := e.task("will fail")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Cancel)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || res.ExitCode != 3 || res.Signal != "" {
		t.Fatalf("result = %+v", res)
	}
	runs := e.fake.Runs(repo)
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.Contains(runs[0].Summary, "exited with code 3") || !strings.Contains(runs[0].Summary, "boom: could not compile") {
		t.Fatalf("run = %+v", runs)
	}
	if got := e.fake.Task(repo, task.ID); got.State != agwp.TaskCancelled {
		t.Fatalf("task = %+v", got)
	}
	if e.metrics.SessionsFinished.With(repo, "bad-bot", "failed").Value() != 1 {
		t.Fatal("failure not counted")
	}
	for _, m := range e.fake.MintedTokens() {
		if !m.Revoked {
			t.Fatal("the credential of a failed session must be revoked")
		}
	}
}

func TestSecretsAreScrubbedFromSummaries(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "chatty-bot", `echo "my token is $AG_TOKEN" >&2; exit 1`)
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("leak"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	token := e.fake.MintedTokens()[0].Secret
	if strings.Contains(res.Summary, token) || strings.Contains(e.fake.Runs(repo)[0].Summary, token) || !strings.Contains(res.Summary, secret.Redacted) {
		t.Fatalf("summary leaks the session token: %q", res.Summary)
	}
}

func TestSessionsDoNotShareCredentials(t *testing.T) {
	e := newEnv(t)
	d := t.TempDir()
	mk := func(name string) config.Executor {
		return commandExec(t, name, `echo "$AG_TOKEN" > "$OUT"; sleep 0.3`, func(c *config.Executor) { c.Env = map[string]string{"OUT": filepath.Join(d, name)} })
	}
	for _, name := range []string{"bot-a", "bot-b"} {
		if _, err := e.mgr.Start(context.Background(), e.request(e.task("t-"+name), mk(name), Keep)); err != nil {
			t.Fatal(err)
		}
	}
	e.wait(10 * time.Second)
	e.wait(10 * time.Second)
	e.idle()
	a, _ := os.ReadFile(filepath.Join(d, "bot-a"))
	b, _ := os.ReadFile(filepath.Join(d, "bot-b"))
	ta, tb := strings.TrimSpace(string(a)), strings.TrimSpace(string(b))
	if ta == "" || tb == "" || ta == tb || ta == e.orchTok || tb == e.orchTok {
		t.Fatalf("tokens: a=%q b=%q", ta, tb)
	}
	// Each token is bound to its own agent: A's token cannot act as B's run.
	cl, _ := agwp.New(e.fake.URL, secret.New(ta), agwp.WithRetry(agwp.RetryPolicy{MaxAttempts: 1}))
	runs := e.fake.Runs(repo)
	var runOfB string
	for _, r := range runs {
		if r.Agent == "bot-b" {
			runOfB = r.RunID
		}
	}
	if _, err := cl.WithRun(runOfB).Whoami(context.Background()); err == nil {
		// whoami ignores ag-run in the fake; prove the boundary through an endpoint that honours it
		_, err = cl.WithRun(runOfB).ListTasks(context.Background(), repo, "", false)
		if err == nil {
			t.Fatal("agent A's credential was accepted for agent B's run")
		}
	}
	// And both are revoked now.
	if _, err := cl.Whoami(context.Background()); err == nil {
		t.Fatal("a finished session's credential still works")
	}
}

func TestKilledExecutorIsReportedAsAbandonedAndLeasesAreReleased(t *testing.T) {
	e := newEnv(t)
	pidFile := filepath.Join(t.TempDir(), "pid")
	ex := commandExec(t, "fragile-bot", `echo $$ > "$PIDFILE"; sleep 30`, func(c *config.Executor) { c.Env = map[string]string{"PIDFILE": pidFile} })
	task := e.task("will be killed", "src/**")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Keep)); err != nil {
		t.Fatal(err)
	}
	var pid int
	deadline := time.Now().Add(5 * time.Second)
	for pid == 0 && time.Now().Before(deadline) {
		if b, err := os.ReadFile(pidFile); err == nil {
			_, _ = fmt.Sscan(strings.TrimSpace(string(b)), &pid)
		}
		time.Sleep(5 * time.Millisecond)
	}
	if pid == 0 {
		t.Fatal("the executor never started")
	}
	if got := e.fake.Leases(repo); len(got) != 1 || got[0].ReleasedAtMs != 0 {
		t.Fatalf("leases while running = %+v", got)
	}
	if err := syscall.Kill(pid, syscall.SIGKILL); err != nil { // not us: someone kills the executor from outside
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || res.Signal != "SIGKILL" {
		t.Fatalf("result = %+v", res)
	}
	run := e.fake.Runs(repo)[0]
	if run.Status != "failed" || !strings.HasPrefix(run.Summary, "abandoned: the executor was killed by signal SIGKILL") {
		t.Fatalf("run = %+v", run)
	}
	for _, l := range e.fake.Leases(repo) {
		if l.ReleasedAtMs == 0 {
			t.Fatalf("lease %+v is still held after the executor died", l)
		}
	}
	if free := e.mgr.FreeSlots(ex); free != ex.Slots {
		t.Fatalf("free slots = %d", free)
	}
}

func TestTimeoutStopsTheWholeProcessGroup(t *testing.T) {
	e := newEnv(t)
	childPid := filepath.Join(t.TempDir(), "child")
	// the shell ignores SIGTERM and runs a background child: both must die
	ex := commandExec(t, "slow-bot", `trap '' TERM; sleep 60 & echo $! > "$CHILD"; wait`, func(c *config.Executor) {
		c.Timeout, c.Grace = 400*time.Millisecond, 200*time.Millisecond
		c.Cred = time.Minute
		c.Env = map[string]string{"CHILD": childPid}
	})
	start := time.Now()
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("slow"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeTimeout || !strings.Contains(res.Summary, "timeout") {
		t.Fatalf("result = %+v", res)
	}
	if took := time.Since(start); took > 5*time.Second {
		t.Fatalf("took %v", took)
	}
	b, _ := os.ReadFile(childPid)
	var pid int
	_, _ = fmt.Sscan(strings.TrimSpace(string(b)), &pid)
	if pid == 0 {
		t.Fatal("child pid unknown")
	}
	deadline := time.Now().Add(3 * time.Second)
	for syscall.Kill(pid, 0) == nil && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if syscall.Kill(pid, 0) == nil {
		t.Fatalf("background child %d survived the session", pid)
	}
	if run := e.fake.Runs(repo)[0]; run.Status != "failed" || !strings.Contains(run.Summary, "timeout") {
		t.Fatalf("run = %+v", run)
	}
}

func TestHeartbeatsKeepTheRunAliveAndExtendLeases(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "steady-bot", `sleep 0.5`)
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("steady", "lib/**"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(10 * time.Second)
	e.idle()
	if n := e.fake.CountRequests("agent.heartbeat"); n < 3 {
		t.Fatalf("heartbeats = %d, want several at 50ms over 0.5s", n)
	}
	if n := e.fake.CountRequests("lease.extend"); n < 3 {
		t.Fatalf("lease extensions = %d", n)
	}
}

func TestCancelStopsAGracefulExecutor(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "polite-bot", `trap 'echo shutting down >&2; exit 0' TERM; while true; do sleep 0.05; done`)
	info, err := e.mgr.Start(context.Background(), e.request(e.task("polite"), ex, Keep))
	if err != nil {
		t.Fatal(err)
	}
	if !e.mgr.Has(info.RunID) || e.mgr.Cancel("run_unknown", "x") {
		t.Fatal("Has/Cancel disagree about ownership")
	}
	time.Sleep(100 * time.Millisecond)
	if !e.mgr.Cancel(info.RunID, "task cancelled by a person") {
		t.Fatal("Cancel did not find the session")
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCancelled || res.Reason != "task cancelled by a person" || res.ExitCode != 0 {
		t.Fatalf("result = %+v", res)
	}
	if run := e.fake.Runs(repo)[0]; run.Status != "failed" || !strings.HasPrefix(run.Summary, "cancelled: task cancelled by a person") {
		t.Fatalf("run = %+v", run)
	}
}

func TestOutputIsCappedAndNotForwarded(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "noisy-bot", `i=0; while [ $i -lt 4000 ]; do echo "line $i ................................................................"; i=$((i+1)); done >&2; exit 2`,
		func(c *config.Executor) { c.Output = 2048 })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("noisy"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed {
		t.Fatalf("result = %+v", res)
	}
	if run := e.fake.Runs(repo)[0]; len(run.Summary) > 1500 || !strings.Contains(run.Summary, "line 3999") {
		t.Fatalf("summary length %d, want a bounded tail ending at the last line: %q", len(run.Summary), run.Summary)
	}
}

func TestSlotLimitsAreEnforced(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "one-at-a-time", `sleep 0.4`, func(c *config.Executor) { c.Slots = 1 })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("first"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	if free := e.mgr.FreeSlots(ex); free != 0 {
		t.Fatalf("free = %d", free)
	}
	t2 := e.task("second")
	if _, err := e.mgr.Start(context.Background(), e.request(t2, ex, Keep)); !errors.Is(err, ErrNoSlot) {
		t.Fatalf("err = %v", err)
	}
	if got := e.fake.Task(repo, t2.ID); got.State != agwp.TaskOpen {
		t.Fatalf("a refused start must not claim the task: %+v", got)
	}
	e.wait(10 * time.Second)
	e.idle()
	if _, err := e.mgr.Start(context.Background(), e.request(t2, ex, Keep)); err != nil {
		t.Fatalf("slot not freed: %v", err)
	}
	e.wait(10 * time.Second)
}

func TestStartRefusesTasksTheServerWillNotRun(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "bot", `true`)
	dep := e.fake.AddTask(repo, "dependency", nil)
	blocked := e.fake.AddTask(repo, "blocked", nil, dep)
	_, err := e.mgr.Start(context.Background(), e.request(e.fake.Task(repo, blocked), ex, Keep))
	if !errors.Is(err, ErrTaskUnavailable) {
		t.Fatalf("err = %v", err)
	}
	if len(e.fake.Runs(repo)) != 0 || len(e.mgr.Active()) != 0 || e.mgr.FreeSlots(ex) != ex.Slots {
		t.Fatal("a refused start left state behind")
	}
}

func TestExclusiveLeaseConflictDefersAndLeavesNothingRunning(t *testing.T) {
	e := newEnv(t)
	e.fake.AddLease(repo, "user:alice", "src/**", true, time.Hour)
	ex := commandExec(t, "bot", `true`)
	task := e.task("needs src", "src/auth/**")
	req := e.request(task, ex, Keep)
	req.Lease = "exclusive"
	_, err := e.mgr.Start(context.Background(), req)
	if !errors.Is(err, ErrLeaseConflict) {
		t.Fatalf("err = %v", err)
	}
	runs := e.fake.Runs(repo)
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.HasPrefix(runs[0].Summary, "deferred:") {
		t.Fatalf("the run started for the failed lease must be completed: %+v", runs)
	}
	if len(e.mgr.Active()) != 0 || e.mgr.FreeSlots(ex) != ex.Slots {
		t.Fatal("state left behind")
	}
	for _, m := range e.fake.MintedTokens() {
		if !m.Revoked {
			t.Fatal("credential not revoked after the failed start")
		}
	}
}

func TestExclusiveLeasesExposeTheirFencingToken(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "bot", `sleep 0.3`)
	req := e.request(e.task("exclusive", "pkg/**"), ex, Keep)
	req.Lease = "exclusive"
	info, err := e.mgr.Start(context.Background(), req)
	if err != nil {
		t.Fatal(err)
	}
	if len(info.Leases) != 1 || !info.Leases[0].Exclusive || info.Leases[0].Token == 0 {
		t.Fatalf("leases = %+v", info.Leases)
	}
	e.wait(10 * time.Second)
}

func TestMismatchedAgentKeyIsRefused(t *testing.T) {
	e := newEnv(t)
	// someone registered "bot" with another key
	other := e.fake.AddToken(fakeagwp.TokenSpec{Identity: "agent:bot"})
	cl, _ := agwp.New(e.fake.URL, secret.New(other))
	otherKey, _ := agentkey.Generate()
	if _, err := cl.RegisterAgent(context.Background(), repo, "bot", otherKey.PublicHex()); err != nil {
		t.Fatal(err)
	}
	_, err := e.mgr.Start(context.Background(), e.request(e.task("t"), commandExec(t, "bot", `true`), Keep))
	if err == nil || !strings.Contains(err.Error(), "different public key") {
		t.Fatalf("err = %v", err)
	}
	if len(e.fake.Runs(repo)) != 0 {
		t.Fatal("no run may start for an agent whose key does not match")
	}
}

func TestAgentIsRegisteredOnceWithTheStoredKey(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "newbie-bot", `true`)
	for i := 0; i < 2; i++ {
		if _, err := e.mgr.Start(context.Background(), e.request(e.task(fmt.Sprint("t", i)), ex, Keep)); err != nil {
			t.Fatal(err)
		}
		e.wait(10 * time.Second)
		e.idle()
	}
	agents := e.fake.Agents(repo)
	if len(agents) != 1 || agents[0].Name != "newbie-bot" {
		t.Fatalf("agents = %+v", agents)
	}
	k, err := agentkey.Load(e.keys.Path("newbie-bot"))
	if err != nil || k.PublicHex() != agents[0].PublicKey {
		t.Fatalf("stored key does not match the registered one: %v", err)
	}
	st, _ := os.Stat(e.keys.Path("newbie-bot"))
	if st.Mode().Perm() != 0o600 {
		t.Fatalf("key mode = %v", st.Mode().Perm())
	}
	if e.fake.CountRequests("agent.register") != 1 {
		t.Fatalf("register calls = %d", e.fake.CountRequests("agent.register"))
	}
}

func TestShutdownWaitLetsShortSessionsFinishAndCancelStopsLongOnes(t *testing.T) {
	e := newEnv(t)
	short := commandExec(t, "short-bot", `sleep 0.2`)
	long := commandExec(t, "long-bot", `sleep 60`)
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("short"), short, Keep)); err != nil {
		t.Fatal(err)
	}
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("long"), long, Keep)); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	e.mgr.Shutdown(ctx, true)
	var shortRes, longRes Result
	for i := 0; i < 2; i++ {
		r := e.wait(5 * time.Second)
		if r.Executor == "short-bot" {
			shortRes = r
		} else {
			longRes = r
		}
	}
	if shortRes.Outcome != OutcomeCompleted {
		t.Fatalf("the short session should have been allowed to finish: %+v", shortRes)
	}
	if longRes.Outcome != OutcomeCancelled || longRes.Reason != "orchestrator shutdown" {
		t.Fatalf("the long session should have been cancelled: %+v", longRes)
	}
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("late"), short, Keep)); !errors.Is(err, ErrClosed) {
		t.Fatalf("err = %v", err)
	}
	for _, r := range e.fake.Runs(repo) {
		if r.Status == "running" {
			t.Fatalf("run %s left running after shutdown", r.RunID)
		}
	}
}

func TestShutdownCancelStopsEverythingImmediately(t *testing.T) {
	e := newEnv(t)
	long := commandExec(t, "long-bot", `sleep 60`)
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("long"), long, Keep)); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	e.mgr.Shutdown(context.Background(), false)
	if r := e.wait(5 * time.Second); r.Outcome != OutcomeCancelled {
		t.Fatalf("result = %+v", r)
	}
	if time.Since(start) > 5*time.Second {
		t.Fatal("cancel-mode shutdown took too long")
	}
}

func TestFinishTaskAsOwnerUsesAShortLivedNarrowCredential(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "owner-bot", `true`)
	task := e.task("owned")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(10 * time.Second)
	e.idle()
	// The service identity itself may not finish an agent's task (server rule) ...
	if _, err := e.orch.FinishTask(context.Background(), repo, task.ID, "done"); err == nil {
		t.Fatal("the fake should mirror the server: only the owner may finish a task")
	}
	// ... the manager does it as the owner.
	if err := e.mgr.FinishTaskAsOwner(context.Background(), repo, "owner-bot", task.ID, "done"); err != nil {
		t.Fatal(err)
	}
	if got := e.fake.Task(repo, task.ID); got.State != agwp.TaskDone {
		t.Fatalf("task = %+v", got)
	}
	var last fakeagwp.MintedInfo
	for _, m := range e.fake.MintedTokens() {
		if len(m.Ops) == 2 { // the session credential carries the executor's whole operation list
			last = m
		}
	}
	if len(last.Ops) != 2 || last.Ops[0] != "task.read" || last.Ops[1] != "task.write" || !last.Revoked || time.Until(last.ExpiresAt) > time.Minute+5*time.Second {
		t.Fatalf("finishing credential = %+v", last)
	}
}

func TestInfoNeverContainsCredentials(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "info-bot", `echo "token=$AG_TOKEN" >&2; sleep 0.5`)
	info, err := e.mgr.Start(context.Background(), e.request(e.task("t", "x/**"), ex, Keep))
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(200 * time.Millisecond)
	snap := fmt.Sprintf("%+v %+v", info, e.mgr.Active())
	token := e.fake.MintedTokens()[0].Secret
	if strings.Contains(snap, token) || strings.Contains(snap, e.orchTok) {
		t.Fatalf("snapshot leaks a credential: %s", snap)
	}
	e.wait(10 * time.Second)
}

func TestExecutorThatCannotLaunchFailsCleanly(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "ghost-bot", ``, func(c *config.Executor) { c.Command = []string{"/definitely/not/a/program"} })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Cancel)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(10 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || !strings.Contains(res.Summary, "cannot start") {
		t.Fatalf("result = %+v", res)
	}
}
