package session

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// These tests run against the real ag-server (internal/testutil). Agents are real
// programs: shell scripts, or the test-only agent program (internal/testutil/testagent)
// that writes files, signs provenance and opens a proposal. A forwarding proxy in front
// of the server records what the manager asks (heartbeats, registrations).

const repo = testutil.RepoName

type env struct {
	t       *testing.T
	w       *testutil.World
	px      *testutil.Proxy
	orch    *agwp.Client
	orchTok string
	mgr     *Manager
	keys    *agentkey.Store
	metrics *metrics.Service
	dir     string
	results chan Result
	granted map[string]bool
}

func newEnv(t *testing.T) *env {
	t.Helper()
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	e := &env{t: t, w: w, dir: t.TempDir(), results: make(chan Result, 16), metrics: metrics.NewService(metrics.NewRegistry()), granted: map[string]bool{}}
	e.px = testutil.NewProxy(t, w.Srv.URL)
	e.orchTok = w.OrchT
	e.orch = w.Srv.ClientAt(e.px.URL, e.orchTok, "orchestrator", agwp.WithObserver(e.metrics))
	var err error
	e.keys, err = agentkey.OpenStore(filepath.Join(e.dir, "keys"))
	if err != nil {
		t.Fatal(err)
	}
	e.mgr, err = NewManager(Options{
		Orch: e.orch, ServerURL: w.Srv.URL, Keys: e.keys, WorkRoot: filepath.Join(e.dir, "work"), Metrics: e.metrics,
		Log:       slog.New(slog.NewTextHandler(io.Discard, nil)),
		ParentEnv: func(k string) string { return map[string]string{"PATH": os.Getenv("PATH"), "LEAKY": "parent-value"}[k] },
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.mgr.Close)
	return e
}

// request builds a session request; the executor's agent gets the writer role first (the operator of a real
// deployment grants it before an agent can be given credentials).
func (e *env) request(task agwp.Task, ex config.Executor, decide Disposition) Request {
	if !e.granted[ex.Agent] {
		if err := e.w.Srv.Admin.GrantRole(context.Background(), "agent:"+ex.Agent, repo, "writer"); err != nil {
			e.t.Fatalf("grant role: %v", err)
		}
		e.granted[ex.Agent] = true
	}
	return Request{
		Repo: repo, Stream: "main", Task: task, Executor: ex, Attempt: 1, Lease: "soft", LeaseTTL: 5 * time.Minute,
		Decide: func(r Result) Disposition { e.results <- r; return decide },
	}
}

func (e *env) task(title string, scope ...string) agwp.Task { return *e.w.Task(title, "", scope) }

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

func (e *env) runs() []agwp.Run {
	e.t.Helper()
	rs, err := e.w.Alice.ListRuns(context.Background(), repo, "", false, 100)
	if err != nil {
		e.t.Fatal(err)
	}
	return rs
}

func (e *env) leases() []agwp.Lease {
	e.t.Helper()
	ls, err := e.w.Alice.ListLeases(context.Background(), repo, false)
	if err != nil {
		e.t.Fatal(err)
	}
	return ls
}

// whoami asks the server who a token is (an error: nobody, it is revoked or unknown).
func (e *env) whoami(token string) (*agwp.Principal, error) {
	c := e.w.Srv.Client(token, "probe", agwp.WithRetry(agwp.RetryPolicy{MaxAttempts: 1}))
	return c.Whoami(context.Background())
}

// waitFile waits until a file written by an executor exists and returns its content.
func (e *env) waitFile(path string) string {
	e.t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if b, err := os.ReadFile(path); err == nil && len(b) > 0 {
			return strings.TrimSpace(string(b))
		}
		time.Sleep(5 * time.Millisecond)
	}
	e.t.Fatalf("%s never appeared", path)
	return ""
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

// agentExec is an executor that runs the test-only agent program.
func agentExec(t *testing.T, name string, files map[string]string, mut ...func(*config.Executor)) config.Executor {
	t.Helper()
	b, err := json.Marshal(files)
	if err != nil {
		t.Fatal(err)
	}
	ex := config.Executor{Name: name, Type: "command", Agent: name, Slots: 2, Timeout: 20 * time.Second, Grace: time.Second,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Command: []string{testutil.BuildTestAgent(t)}, Pass: []string{"PATH"},
		Env: map[string]string{"TESTAGENT_FILES": string(b), "TESTAGENT_MESSAGE": "test agent change", "TESTAGENT_TITLE": "Test agent change",
			"TESTAGENT_MODEL": "test-agent", "TESTAGENT_RATIONALE": "because"}}
	for _, f := range mut {
		f(&ex)
	}
	return ex
}

func TestAgentSessionProposesSignsAndCompletes(t *testing.T) {
	e := newEnv(t)
	e.w.Seed(map[string]string{"README.md": "hi\n"})
	task := e.task("Add docs", "docs/**")
	info, err := e.mgr.Start(context.Background(), e.request(task, agentExec(t, "docs-bot", map[string]string{"docs/guide.md": "# Guide\n"}), Keep))
	if err != nil {
		t.Fatal(err)
	}
	if info.State != "running" || info.Agent != "docs-bot" || len(info.Leases) != 1 || info.Leases[0].Scope != "docs/**" || info.Leases[0].Exclusive {
		t.Fatalf("info = %+v", info)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCompleted || len(res.Proposals) != 1 {
		t.Fatalf("result = %+v", res)
	}
	p := res.Proposals[0]
	if p.Author != "agent:docs-bot#"+res.RunID || p.TaskID != task.ID || p.State != agwp.ProposalOpen {
		t.Fatalf("proposal = %+v", p)
	}
	// provenance was submitted by the agent for exactly the proposed head, the server verified its signature,
	// and the file is in the workspace head
	prov, err := e.w.Alice.ListProvenance(context.Background(), repo, p.Head)
	if err != nil {
		t.Fatal(err)
	}
	if len(prov) != 1 || !prov[0].Verified || prov[0].Record.Changeset != p.Head || prov[0].Record.TaskID != task.ID || prov[0].Record.Agent != p.Author || len(prov[0].Signature) != 128 {
		t.Fatalf("provenance = %+v", prov)
	}
	if b, ok := e.w.File(p.Head, "docs/guide.md"); !ok || b != "# Guide\n" {
		t.Fatalf("file = %q %v", b, ok)
	}
	// run completed, lease released by the run's completion, credential revoked, task left in progress
	runs := e.runs()
	if len(runs) != 1 || runs[0].Status != "completed" || !strings.Contains(runs[0].Summary, p.ID) {
		t.Fatalf("runs = %+v", runs)
	}
	for _, l := range e.leases() {
		if l.ReleasedAtMs == 0 {
			t.Fatalf("lease %+v still held", l)
		}
	}
	creds := e.w.Srv.Credentials("agent:docs-bot")
	if len(creds) != 1 || creds[0].RevokedAtMs == 0 {
		t.Fatalf("the session credential must be revoked: %+v", creds)
	}
	if got := e.w.GetTask(task.ID); got.State != agwp.TaskInProgress || got.Owner != "agent:docs-bot" {
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
	e.w.Seed(map[string]string{"a": "b"})
	ex := agentExec(t, "scoped-bot", map[string]string{"x.txt": "x"}, func(c *config.Executor) { c.Cred = 7 * time.Minute })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	minted := e.w.Srv.Credentials("agent:scoped-bot")
	if len(minted) != 1 {
		t.Fatalf("minted = %+v", minted)
	}
	m := minted[0]
	if m.Subject != "agent:scoped-bot" || len(m.Scope.Repos) != 1 || m.Scope.Repos[0] != repo || len(m.Scope.Streams) != 1 || m.Scope.Streams[0] != "main" || m.Scope.RunID == "" {
		t.Fatalf("scope = %+v", m)
	}
	for _, op := range m.Scope.Operations {
		if config.ForbiddenAgentOps[op] {
			t.Fatalf("the agent credential carries %q", op)
		}
	}
	if ttl := time.Duration(m.ExpiresAtMs-m.CreatedAtMs) * time.Millisecond; ttl < 7*time.Minute-5*time.Second || ttl > 7*time.Minute+5*time.Second {
		t.Fatalf("ttl = %v", ttl)
	}
}

func TestCommandExecutorGetsExactlyTheDocumentedEnvironment(t *testing.T) {
	e := newEnv(t)
	dir := t.TempDir()
	dump, release := filepath.Join(dir, "env.txt"), filepath.Join(dir, "release")
	ex := commandExec(t, "env-bot", `env | sort > "$DUMP.tmp"; cat "$AG_TASK_FILE" > "$DUMP.task"; ls -l "$AG_KEY_FILE" | cut -c1-10 > "$DUMP.keymode"; echo '{"summary":"dumped the environment"}' > "$AG_RESULT_FILE"; mv "$DUMP.tmp" "$DUMP"
while [ ! -f "$RELEASE" ]; do sleep 0.02; done`,
		func(c *config.Executor) {
			c.Env = map[string]string{"DUMP": dump, "RELEASE": release}
			c.Pass = []string{"PATH", "LEAKY_NOT_LISTED"}
		})
	source := e.task("source work", "src/**")
	status, body := e.w.Srv.Call(e.w.AliceT, "POST", "/v1/repos/"+repo+"/tasks", map[string]any{
		"title": "env check", "scope": []string{"src/**"}, "source_task": source.ID,
	})
	if status != 200 {
		t.Fatalf("create follow-up: %d %s", status, body)
	}
	var task agwp.Task
	if err := json.Unmarshal(body, &task); err != nil {
		t.Fatal(err)
	}
	// The dispatcher may have selected this task before its instructions were refined.
	status, body = e.w.Srv.Call(e.w.AliceT, "PATCH", "/v1/repos/"+repo+"/tasks/"+task.ID, map[string]any{
		"expected": map[string]any{"title": task.Title, "description": task.Description, "scope": task.Scope},
		"content":  map[string]any{"title": "env check", "description": "Use the refined instructions", "scope": []string{"src/refined/**"}},
	})
	if status != 200 {
		t.Fatalf("refine queued task: %d %s", status, body)
	}
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Done)); err != nil {
		t.Fatal(err)
	}
	raw := e.waitFile(dump)
	vars := map[string]string{}
	for _, line := range strings.Split(raw, "\n") {
		k, v, _ := strings.Cut(line, "=")
		vars[k] = v
	}
	// While the session runs, the executor's token is the agent's own run-bound credential on the server.
	p, err := e.whoami(vars["AG_TOKEN"])
	if err != nil {
		t.Fatalf("the executor's token does not work: %v", err)
	}
	if p.Identity != "agent:env-bot" || p.Scope.RunID != vars["AG_RUN"] || len(p.Scope.Repos) != 1 || p.Scope.Repos[0] != repo {
		t.Fatalf("the executor's token is %+v", p)
	}
	if err := os.WriteFile(release, []byte("go"), 0o600); err != nil {
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCompleted || !strings.Contains(res.Summary, "dumped the environment") {
		t.Fatalf("result = %+v", res)
	}
	for k, want := range map[string]string{
		"AG_SERVER": e.w.Srv.URL, "AG_RUN": res.RunID, "AG_REPO": repo, "AG_STREAM": "main", "AG_TASK": task.ID,
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
	if strings.Contains(raw, e.orchTok) {
		t.Fatal("the orchestrator's own credential reached the executor")
	}
	if mode, _ := os.ReadFile(dump + ".keymode"); !strings.HasPrefix(string(mode), "-rw-------") {
		t.Errorf("key file mode = %q", mode)
	}
	tf, err := os.ReadFile(dump + ".task")
	if err != nil || !strings.Contains(string(tf), "env check") {
		t.Fatalf("task file = %s, error = %v", tf, err)
	}
	var delivered map[string]any
	if err := json.Unmarshal(tf, &delivered); err != nil {
		t.Fatal(err)
	}
	if delivered["description"] != "Use the refined instructions" {
		t.Fatalf("executor received stale instructions: %v", delivered["description"])
	}
	paths, ok := delivered["scope"].([]any)
	if !ok || len(paths) != 1 || paths[0] != "src/refined/**" {
		t.Fatalf("executor received stale scope: %v", delivered["scope"])
	}
	if delivered["source_task"] != source.ID {
		t.Fatalf("executor lost the task source: %v", delivered["source_task"])
	}
	// Done disposition: the owner (the agent) finished the task.
	if got := e.w.GetTask(task.ID); got.State != agwp.TaskDone {
		t.Fatalf("task = %+v", got)
	}
	if _, err := os.Stat(filepath.Join(e.dir, "work", res.RunID)); !os.IsNotExist(err) {
		t.Fatal("the session work directory was not removed")
	}
	if _, err := e.whoami(vars["AG_TOKEN"]); err == nil {
		t.Fatal("the credential of a finished session still works")
	}
}

func TestFailingExecutorFailsTheRunAndCancelsTheTask(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "bad-bot", `echo "working..."; echo "boom: could not compile" >&2; exit 3`)
	task := e.task("will fail")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Cancel)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || res.ExitCode != 3 || res.Signal != "" {
		t.Fatalf("result = %+v", res)
	}
	runs := e.runs()
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.Contains(runs[0].Summary, "exited with code 3") || !strings.Contains(runs[0].Summary, "boom: could not compile") {
		t.Fatalf("run = %+v", runs)
	}
	if got := e.w.GetTask(task.ID); got.State != agwp.TaskCancelled {
		t.Fatalf("task = %+v", got)
	}
	if e.metrics.SessionsFinished.With(repo, "bad-bot", "failed").Value() != 1 {
		t.Fatal("failure not counted")
	}
	for _, c := range e.w.Srv.Credentials("agent:bad-bot") {
		if c.RevokedAtMs == 0 {
			t.Fatal("the credential of a failed session must be revoked")
		}
	}
}

func TestSecretsAreScrubbedFromSummaries(t *testing.T) {
	e := newEnv(t)
	tokFile := filepath.Join(t.TempDir(), "token")
	ex := commandExec(t, "chatty-bot", `echo "$AG_TOKEN" > "$OUT"; echo "my token is $AG_TOKEN" >&2; exit 1`, func(c *config.Executor) { c.Env = map[string]string{"OUT": tokFile} })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("leak"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	token := e.waitFile(tokFile)
	if strings.Contains(res.Summary, token) || strings.Contains(e.runs()[0].Summary, token) || !strings.Contains(res.Summary, secret.Redacted) {
		t.Fatalf("summary leaks the session token: %q", res.Summary)
	}
}

func TestSessionsDoNotShareCredentials(t *testing.T) {
	e := newEnv(t)
	d := t.TempDir()
	release := filepath.Join(d, "release")
	mk := func(name string) config.Executor {
		return commandExec(t, name, `echo "$AG_TOKEN" > "$OUT.tmp"; mv "$OUT.tmp" "$OUT"; while [ ! -f "$RELEASE" ]; do sleep 0.02; done`,
			func(c *config.Executor) { c.Env = map[string]string{"OUT": filepath.Join(d, name), "RELEASE": release} })
	}
	for _, name := range []string{"bot-a", "bot-b"} {
		if _, err := e.mgr.Start(context.Background(), e.request(e.task("t-"+name), mk(name), Keep)); err != nil {
			t.Fatal(err)
		}
	}
	ta, tb := e.waitFile(filepath.Join(d, "bot-a")), e.waitFile(filepath.Join(d, "bot-b"))
	if ta == tb || ta == e.orchTok || tb == e.orchTok {
		t.Fatalf("tokens: a=%q b=%q", ta, tb)
	}
	// Each token is bound to its own agent's run: A's token works for A's run and cannot act as B's run.
	var runOfA, runOfB string
	for _, r := range e.runs() {
		switch r.Agent {
		case "bot-a":
			runOfA = r.RunID
		case "bot-b":
			runOfB = r.RunID
		}
	}
	cl := e.w.Srv.Client(ta, "agent-a", agwp.WithRetry(agwp.RetryPolicy{MaxAttempts: 1}))
	if _, err := cl.WithRun(runOfA).ListTasks(context.Background(), repo, "", false); err != nil {
		t.Fatalf("A's credential does not work for A's run: %v", err)
	}
	if _, err := cl.WithRun(runOfB).ListTasks(context.Background(), repo, "", false); err == nil {
		t.Fatal("agent A's credential was accepted for agent B's run")
	}
	if err := os.WriteFile(release, []byte("go"), 0o600); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.wait(20 * time.Second)
	e.idle()
	// And both are revoked now.
	for _, tok := range []string{ta, tb} {
		if _, err := e.whoami(tok); err == nil {
			t.Fatal("a finished session's credential still works")
		}
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
	_, _ = fmt.Sscan(e.waitFile(pidFile), &pid)
	if pid == 0 {
		t.Fatal("the executor never started")
	}
	if got := e.leases(); len(got) != 1 || got[0].ReleasedAtMs != 0 {
		t.Fatalf("leases while running = %+v", got)
	}
	if err := syscall.Kill(pid, syscall.SIGKILL); err != nil { // not us: someone kills the executor from outside
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || res.Signal != "SIGKILL" {
		t.Fatalf("result = %+v", res)
	}
	run := e.runs()[0]
	if run.Status != "failed" || !strings.HasPrefix(run.Summary, "abandoned: the executor was killed by signal SIGKILL") {
		t.Fatalf("run = %+v", run)
	}
	for _, l := range e.leases() {
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
	res := e.wait(20 * time.Second)
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
	for !processExited(pid) && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if !processExited(pid) {
		t.Fatalf("background child %d survived the session", pid)
	}
	if run := e.runs()[0]; run.Status != "failed" || !strings.Contains(run.Summary, "timeout") {
		t.Fatalf("run = %+v", run)
	}
}

// A dead, unreaped child still answers kill(0) in containers whose PID 1 does not reap orphans.
// Count that as exited, while continuing to reject a running or stopped child.
func processExited(pid int) bool {
	err := syscall.Kill(pid, 0)
	if err == syscall.ESRCH {
		return true
	}
	if err != nil || runtime.GOOS != "linux" {
		return false
	}
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return errors.Is(err, os.ErrNotExist)
	}
	i := strings.LastIndex(string(b), ") ")
	return i >= 0 && len(b) > i+2 && b[i+2] == 'Z'
}

func TestHeartbeatsKeepTheRunAliveAndExtendLeases(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "steady-bot", `sleep 0.5`)
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("steady", "lib/**"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	if n := e.px.Count("POST", "/heartbeat"); n < 3 {
		t.Fatalf("heartbeats = %d, want several at 50ms over 0.5s", n)
	}
	if n := e.px.Count("POST", "/extend"); n < 3 {
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
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeCancelled || res.Reason != "task cancelled by a person" || res.ExitCode != 0 {
		t.Fatalf("result = %+v", res)
	}
	if run := e.runs()[0]; run.Status != "failed" || !strings.HasPrefix(run.Summary, "cancelled: task cancelled by a person") {
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
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed {
		t.Fatalf("result = %+v", res)
	}
	if run := e.runs()[0]; len(run.Summary) > 1500 || !strings.Contains(run.Summary, "line 3999") {
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
	if got := e.w.GetTask(t2.ID); got.State != agwp.TaskOpen {
		t.Fatalf("a refused start must not claim the task: %+v", got)
	}
	e.wait(20 * time.Second)
	e.idle()
	if _, err := e.mgr.Start(context.Background(), e.request(t2, ex, Keep)); err != nil {
		t.Fatalf("slot not freed: %v", err)
	}
	e.wait(20 * time.Second)
}

func TestStartRefusesTasksTheServerWillNotRun(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "bot", `true`)
	dep := e.w.Task("dependency", "", nil)
	blocked := e.w.Task("blocked", "", nil, dep.ID)
	_, err := e.mgr.Start(context.Background(), e.request(*blocked, ex, Keep))
	if !errors.Is(err, ErrTaskUnavailable) {
		t.Fatalf("err = %v", err)
	}
	if len(e.runs()) != 0 || len(e.mgr.Active()) != 0 || e.mgr.FreeSlots(ex) != ex.Slots {
		t.Fatal("a refused start left state behind")
	}
}

func TestExclusiveLeaseConflictDefersAndLeavesNothingRunning(t *testing.T) {
	e := newEnv(t)
	if _, err := e.w.Alice.AcquireLease(context.Background(), repo, "src/**", true, time.Hour, ""); err != nil {
		t.Fatal(err)
	}
	ex := commandExec(t, "bot", `true`)
	task := e.task("needs src", "src/auth/**")
	req := e.request(task, ex, Keep)
	req.Lease = "exclusive"
	_, err := e.mgr.Start(context.Background(), req)
	if !errors.Is(err, ErrLeaseConflict) {
		t.Fatalf("err = %v", err)
	}
	runs := e.runs()
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.HasPrefix(runs[0].Summary, "deferred:") {
		t.Fatalf("the run started for the failed lease must be completed: %+v", runs)
	}
	if len(e.mgr.Active()) != 0 || e.mgr.FreeSlots(ex) != ex.Slots {
		t.Fatal("state left behind")
	}
	for _, c := range e.w.Srv.Credentials("agent:bot") {
		if c.RevokedAtMs == 0 {
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
	e.wait(20 * time.Second)
}

func TestMismatchedAgentKeyIsRefused(t *testing.T) {
	e := newEnv(t)
	// someone registered "bot" with another key
	if err := e.w.Srv.Admin.GrantRole(context.Background(), "agent:bot", repo, "writer"); err != nil {
		t.Fatal(err)
	}
	e.granted["bot"] = true
	cl := e.w.Srv.Client(e.w.Mint("agent:bot", nil), "agent")
	otherKey, _ := agentkey.Generate()
	if _, err := cl.RegisterAgent(context.Background(), repo, "bot", otherKey.PublicHex()); err != nil {
		t.Fatal(err)
	}
	_, err := e.mgr.Start(context.Background(), e.request(e.task("t"), commandExec(t, "bot", `true`), Keep))
	if err == nil || !strings.Contains(err.Error(), "different public key") {
		t.Fatalf("err = %v", err)
	}
	if len(e.runs()) != 0 {
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
		e.wait(20 * time.Second)
		e.idle()
	}
	agents, err := e.w.Alice.ListAgents(context.Background(), repo)
	if err != nil {
		t.Fatal(err)
	}
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
	if n := e.px.Count("POST", "/agents"); n != 1 {
		t.Fatalf("register calls = %d", n)
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
		r := e.wait(10 * time.Second)
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
	for _, r := range e.runs() {
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
	if r := e.wait(10 * time.Second); r.Outcome != OutcomeCancelled {
		t.Fatalf("result = %+v", r)
	}
	if time.Since(start) > 5*time.Second {
		t.Fatal("cancel-mode shutdown took too long")
	}
}

func TestAnOperatorFinishesTheTaskOfAnAgentWithoutMintingAnything(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "owner-bot", `true`)
	task := e.task("owned")
	if _, err := e.mgr.Start(context.Background(), e.request(task, ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	minted := len(e.w.Srv.Credentials(""))
	// The service is an operator (the delegator role): it finishes the agent's task itself ...
	if err := e.mgr.FinishTask(context.Background(), repo, task.ID, "done"); err != nil {
		t.Fatal(err)
	}
	if got := e.w.GetTask(task.ID); got.State != agwp.TaskDone {
		t.Fatalf("task = %+v", got)
	}
	// ... without a credential made for the occasion (it used to mint one for the owner and revoke it again).
	if n := len(e.w.Srv.Credentials("")); n != minted {
		t.Fatalf("finishing a task minted %d credential(s)", n-minted)
	}
	// A service that is no operator still may not.
	if err := e.w.Srv.Admin.GrantRole(context.Background(), "service:plain", repo, "writer"); err != nil {
		t.Fatal(err)
	}
	c := e.w.Srv.Client(e.w.Mint("service:plain", nil), "plain")
	other := e.task("someone else's")
	if _, err := e.mgr.Start(context.Background(), e.request(other, ex, Keep)); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	if _, err := c.FinishTask(context.Background(), repo, other.ID, "done"); err == nil {
		t.Fatal("a writer must not finish an agent's task")
	}
}

func TestEachSessionGetsACredentialBoundToItsRunThatEndsWithIt(t *testing.T) {
	e := newEnv(t)
	dir := t.TempDir()
	tokFile, release := filepath.Join(dir, "token"), filepath.Join(dir, "release")
	ex := commandExec(t, "bound-bot", `echo "$AG_TOKEN" > "$OUT.tmp"; mv "$OUT.tmp" "$OUT"; while [ ! -f "$RELEASE" ]; do sleep 0.02; done`,
		func(c *config.Executor) { c.Env = map[string]string{"OUT": tokFile, "RELEASE": release} })
	info, err := e.mgr.Start(context.Background(), e.request(e.task("t", "x/**"), ex, Keep))
	if err != nil {
		t.Fatal(err)
	}
	token := e.waitFile(tokFile)
	minted := e.w.Srv.Credentials("agent:bound-bot")
	if len(minted) != 1 {
		t.Fatalf("minted = %+v", minted)
	}
	if minted[0].Scope.RunID != info.RunID || len(minted[0].Scope.Repos) != 1 || minted[0].Scope.Repos[0] != repo {
		t.Fatalf("the session credential is not bound to run %s in %s: %+v", info.RunID, repo, minted[0])
	}
	if _, err := e.whoami(token); err != nil {
		t.Fatalf("the credential does not work while the run is running: %v", err)
	}
	if err := os.WriteFile(release, []byte("go"), 0o600); err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	if _, err := e.whoami(token); err == nil {
		t.Fatal("the credential of a finished session must be dead")
	}
}

func TestInfoNeverContainsCredentials(t *testing.T) {
	e := newEnv(t)
	tokFile := filepath.Join(t.TempDir(), "token")
	ex := commandExec(t, "info-bot", `echo "$AG_TOKEN" > "$OUT"; echo "token=$AG_TOKEN" >&2; sleep 0.5`, func(c *config.Executor) { c.Env = map[string]string{"OUT": tokFile} })
	info, err := e.mgr.Start(context.Background(), e.request(e.task("t", "x/**"), ex, Keep))
	if err != nil {
		t.Fatal(err)
	}
	token := e.waitFile(tokFile)
	time.Sleep(200 * time.Millisecond)
	snap := fmt.Sprintf("%+v %+v", info, e.mgr.Active())
	if strings.Contains(snap, token) || strings.Contains(snap, e.orchTok) {
		t.Fatalf("snapshot leaks a credential: %s", snap)
	}
	e.wait(20 * time.Second)
}

func TestExecutorThatCannotLaunchFailsCleanly(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "ghost-bot", ``, func(c *config.Executor) { c.Command = []string{"/definitely/not/a/program"} })
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Cancel)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || !strings.Contains(res.Summary, "cannot start") {
		t.Fatalf("result = %+v", res)
	}
}

// An agent credential without workspace.write cannot even start a change: the server's refusal reaches the run summary.
func TestAgentReportsServerRefusals(t *testing.T) {
	e := newEnv(t)
	e.w.Seed(map[string]string{"a": "b"})
	ex := agentExec(t, "weak-bot", map[string]string{"x": "y"}, func(c *config.Executor) {
		c.Ops = []string{"repo.read", "task.read", "agent.run", "lease.read", "lease.write"}
	})
	if _, err := e.mgr.Start(context.Background(), e.request(e.task("t"), ex, Keep)); err != nil {
		t.Fatal(err)
	}
	res := e.wait(20 * time.Second)
	e.idle()
	if res.Outcome != OutcomeFailed || !strings.Contains(res.Summary, "create workspace") || !strings.Contains(res.Summary, "permission_denied") {
		t.Fatalf("result = %+v", res)
	}
}

func TestOptedInOutputIsSavedWithKnownSecretsRemoved(t *testing.T) {
	e := newEnv(t)
	ex := commandExec(t, "output-bot", `printf '%s\n' "$AG_TOKEN" "$LEAKY"; cat "$AG_KEY_FILE"; printf 'diagnostic failure\n' >&2; exit 2`, func(c *config.Executor) { c.PublishOutput = true; c.Pass = append(c.Pass, "LEAKY") })
	info, err := e.mgr.Start(context.Background(), e.request(e.task("capture output"), ex, Keep))
	if err != nil {
		t.Fatal(err)
	}
	e.wait(20 * time.Second)
	e.idle()
	out, err := e.orch.GetRunOutput(context.Background(), repo, info.RunID)
	if err != nil {
		t.Fatal(err)
	}
	if !out.Available || !strings.Contains(out.Stderr, "diagnostic failure") {
		t.Fatal("expected captured output and failure diagnostic")
	}
	if strings.Contains(out.Stdout, "parent-value") || strings.Contains(out.Stdout, "agt_") {
		t.Fatal("saved output contains known credentials")
	}
	var key struct {
		Secret string `json:"secret"`
	}
	raw, err := os.ReadFile(e.keys.Path("output-bot"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, &key); err != nil {
		t.Fatal(err)
	}
	if key.Secret == "" || strings.Contains(out.Stdout, key.Secret) {
		t.Fatal("saved output contains the private signing key")
	}
}
