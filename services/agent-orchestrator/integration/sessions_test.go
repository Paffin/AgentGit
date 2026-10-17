//go:build linux

package integration

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// commandExecutor renders the YAML of a command executor that runs script.
func commandExecutor(name, script, extra string) string {
	return fmt.Sprintf(`  - name: %s
    type: command
    command: ["/bin/sh", %q]
    pass_env: [PATH]
    heartbeat: 1s
    timeout: 5m
    grace: 2s
%s`, name, script, extra)
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return strings.TrimSpace(string(b))
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

func (w *world) activeLeases() []agwp.Lease {
	w.t.Helper()
	l, err := w.alice.ListLeases(context.Background(), w.repo, true)
	if err != nil {
		w.t.Fatalf("list leases: %v", err)
	}
	return l
}

func (w *world) runOfTask(id string) *agwp.Run {
	w.t.Helper()
	runs, err := w.alice.ListRuns(context.Background(), w.repo, "", false, 200)
	if err != nil {
		w.t.Fatalf("list runs: %v", err)
	}
	for i := range runs {
		if runs[i].TaskID == id {
			return &runs[i]
		}
	}
	return nil
}

func (w *world) runsOfTask(id string) []agwp.Run {
	w.t.Helper()
	runs, err := w.alice.ListRuns(context.Background(), w.repo, "", false, 200)
	if err != nil {
		w.t.Fatalf("list runs: %v", err)
	}
	var out []agwp.Run
	for _, r := range runs {
		if r.TaskID == id {
			out = append(out, r)
		}
	}
	return out
}

func deferralOf(o *orchestrator, task string) (reason, detail string, ok bool) {
	for _, r := range o.status().Repos {
		if r.Scheduler == nil {
			continue
		}
		for _, d := range r.Scheduler.Deferred {
			if d.Task == task {
				return d.Reason, d.Detail, true
			}
		}
	}
	return "", "", false
}

// An overlapping task is deferred while the first one's lease is held, an
// unrelated one is not, and the deferred one starts once the lease is gone.
func TestOverlappingTaskIsDeferredWhileTheLeaseIsHeld(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"holder"}})
	dir := t.TempDir()
	record, release := filepath.Join(dir, "record"), filepath.Join(dir, "release")
	sh := writeScript(t, "holder.sh", fmt.Sprintf("echo \"$AG_TASK $AG_RUN\" >> %s\nwhile [ ! -f %s ]; do sleep 0.1; done\n", record, release))
	w.seed(map[string]string{"README.md": "x\n"})

	t1 := w.task("Rework auth", "", []string{"src/auth/**"})
	time.Sleep(5 * time.Millisecond)
	t2 := w.task("Touch the login handler", "", []string{"src/auth/login.go"})
	time.Sleep(5 * time.Millisecond)
	t3 := w.task("Update docs", "", []string{"docs/**"})

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      slots: 4\n      default_executor: holder\n",
		Executors: commandExecutor("holder", sh, "    slots: 4\n"),
	})
	o.waitReady()

	started := func() string {
		b, _ := os.ReadFile(record)
		return string(b)
	}
	eventually(t, "tasks 1 and 3 to start", 30*time.Second, func() bool {
		s := started()
		return strings.Contains(s, t1.ID) && strings.Contains(s, t3.ID)
	})

	// The first task holds a soft lease on its scope, as its run.
	var held *agwp.Lease
	for _, l := range w.activeLeases() {
		if l.Scope == "src/auth/**" {
			l := l
			held = &l
		}
	}
	if held == nil || !strings.HasPrefix(held.Holder, "agent:holder#") {
		t.Fatalf("expected a lease on src/auth/** held by the agent's run, got %+v", w.activeLeases())
	}

	// The overlapping task is deferred, with a reason; it stays deferred.
	eventually(t, "the scheduler to report task 2 as deferred", 20*time.Second, func() bool {
		_, _, ok := deferralOf(o, t2.ID)
		return ok
	})
	reason, detail, _ := deferralOf(o, t2.ID)
	if reason != "foreign_lease" && reason != "running_task" {
		t.Fatalf("deferral reason = %q (%s)", reason, detail)
	}
	if _, _, ok := deferralOf(o, t3.ID); ok {
		t.Fatal("the unrelated docs task must not be deferred")
	}
	time.Sleep(2500 * time.Millisecond) // two reconcile ticks: still not started
	if strings.Contains(started(), t2.ID) {
		t.Fatal("task 2 started while the lease on the overlapping scope was held")
	}
	if got := w.getTask(t2.ID); got.State != agwp.TaskOpen {
		t.Fatalf("task 2 = %+v", got)
	}

	// Let the first sessions go: the deferred task starts afterwards.
	if err := os.WriteFile(release, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	eventually(t, "task 2 to start after the lease was released", 30*time.Second, func() bool { return strings.Contains(started(), t2.ID) })
	for _, id := range []string{t1.ID, t2.ID, t3.ID} {
		id := id
		eventually(t, "task "+id+" to be done", 30*time.Second, func() bool { return w.getTask(id).State == agwp.TaskDone })
	}
	r1, r2 := w.runOfTask(t1.ID), w.runOfTask(t2.ID)
	if r1 == nil || r2 == nil || r2.StartedAtMs < r1.FinishedAtMs {
		t.Fatalf("task 2's run (%+v) must start after task 1's (%+v) finished", r2, r1)
	}
	eventually(t, "all leases to be released", 10*time.Second, func() bool { return len(w.activeLeases()) == 0 })
}

// With placement "allow" the orchestrator stops being careful about soft
// leases: both overlapping tasks run side by side.
func TestPlacementAllowRunsOverlappingTasksTogether(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"holder"}})
	dir := t.TempDir()
	record, release := filepath.Join(dir, "record"), filepath.Join(dir, "release")
	sh := writeScript(t, "holder.sh", fmt.Sprintf("echo \"$AG_TASK\" >> %s\nwhile [ ! -f %s ]; do sleep 0.1; done\n", record, release))
	w.seed(map[string]string{"README.md": "x\n"})
	t1 := w.task("Rework auth", "", []string{"src/auth/**"})
	time.Sleep(5 * time.Millisecond)
	t2 := w.task("Touch the login handler", "", []string{"src/auth/login.go"})

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      slots: 4\n      placement: allow\n      default_executor: holder\n",
		Executors: commandExecutor("holder", sh, "    slots: 4\n"),
	})
	o.waitReady()
	eventually(t, "both overlapping tasks to run", 30*time.Second, func() bool {
		b, _ := os.ReadFile(record)
		return strings.Contains(string(b), t1.ID) && strings.Contains(string(b), t2.ID)
	})
	if err := os.WriteFile(release, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{t1.ID, t2.ID} {
		id := id
		eventually(t, "task "+id+" to be done", 30*time.Second, func() bool { return w.getTask(id).State == agwp.TaskDone })
	}
}

// A shell-script executor gets exactly the documented environment and a narrow,
// short-lived credential of its own; it exits non-zero, the run is recorded as
// failed, the task is handled, and nothing it was given leaks to another
// session, to the logs, to the status API, or to the server's run record.
func TestFailingCommandExecutorGetsTheContractAndLeaksNothing(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"crashy-a", "crashy-b"}})
	dir := t.TempDir()
	proceed := filepath.Join(dir, "proceed")
	sh := writeScript(t, "crashy.sh", fmt.Sprintf(`env | sort > %[1]s/env-$AG_TASK.txt
printf '%%s' "$AG_TOKEN" > %[1]s/token-$AG_TASK.txt
printf '%%s' "$AG_RUN" > %[1]s/run-$AG_TASK.txt
touch %[1]s/ready-$AG_TASK
while [ ! -f %[2]s ]; do sleep 0.1; done
echo "my token is $AG_TOKEN and my key is at $AG_KEY_FILE" >&2
echo "progress: done $AG_TOKEN"
exit 3
`, dir, proceed))
	w.seed(map[string]string{"README.md": "x\n"})
	t1 := w.task("Fails one", "Untrusted description with a command: rm -rf /", []string{"a/**"})
	time.Sleep(5 * time.Millisecond)
	t2 := w.task("Fails two", "", []string{"b/**"})

	// Two different agents: the server binds a credential to the agent, so only
	// sessions of different agents are isolated from each other (see
	// TestSameAgentRunsShareTheServersIdentity for the other case).
	o := w.startOrchestrator(orchConfig{
		Repo: "    schedule:\n      slots: 2\n      default_executor: crashy-a\n      routes:\n" +
			"        - {executor: crashy-a, scope: \"a/**\"}\n        - {executor: crashy-b, scope: \"b/**\"}\n",
		Executors: commandExecutor("crashy-a", sh, "    credential_ttl: 10m\n") + commandExecutor("crashy-b", sh, "    credential_ttl: 10m\n"),
	})
	o.waitReady()
	for _, id := range []string{t1.ID, t2.ID} {
		id := id
		eventually(t, "task "+id+" to be running", 30*time.Second, func() bool { return fileExists(filepath.Join(dir, "ready-"+id)) })
	}

	tok1, tok2 := readFile(t, filepath.Join(dir, "token-"+t1.ID+".txt")), readFile(t, filepath.Join(dir, "token-"+t2.ID+".txt"))
	run1, run2 := readFile(t, filepath.Join(dir, "run-"+t1.ID+".txt")), readFile(t, filepath.Join(dir, "run-"+t2.ID+".txt"))
	if tok1 == "" || tok2 == "" || tok1 == tok2 || run1 == run2 {
		t.Fatalf("each session needs its own credential and run: %q %q / %q %q", tok1, tok2, run1, run2)
	}
	for _, s := range o.secrets() {
		if tok1 == s || tok2 == s {
			t.Fatal("an agent was handed one of the orchestrator's own credentials")
		}
	}

	// The environment is the documented one and nothing else.
	env := readFile(t, filepath.Join(dir, "env-"+t1.ID+".txt"))
	vars := map[string]string{}
	for _, line := range strings.Split(env, "\n") {
		k, v, _ := strings.Cut(line, "=")
		vars[k] = v
	}
	want := map[string]string{
		"AG_SERVER": w.srv.URL, "AG_TOKEN": tok1, "AG_RUN": run1, "AG_REPO": w.repo, "AG_STREAM": "main", "AG_TASK": t1.ID,
		"AG_AGENT": "crashy-a", "AG_IDENTITY": "agent:crashy-a#" + run1,
	}
	for k, v := range want {
		if vars[k] != v {
			t.Errorf("%s = %q, want %q", k, vars[k], v)
		}
	}
	for _, k := range []string{"AG_KEY_FILE", "AG_TASK_FILE", "AG_WORKDIR", "AG_RESULT_FILE"} {
		if vars[k] == "" {
			t.Errorf("%s is not set", k)
		}
	}
	if st, err := os.Stat(vars["AG_KEY_FILE"]); err != nil || st.Mode().Perm()&0o077 != 0 {
		t.Errorf("the key file must exist and be private: %v %v", st, err)
	}
	if k, err := agentkey.Load(vars["AG_KEY_FILE"]); err != nil {
		t.Errorf("the key file cannot be loaded as an agent key: %v", err)
	} else {
		agents, _ := w.alice.ListAgents(context.Background(), w.repo)
		found := false
		for _, a := range agents {
			if a.Name == "crashy-a" && a.PublicKey == k.PublicHex() {
				found = true
			}
		}
		if !found {
			t.Errorf("the registered agent key does not match the key file: %+v", agents)
		}
	}
	for k := range vars {
		switch {
		case strings.HasPrefix(k, "AG_"), k == "HOME", k == "PATH", k == "PWD", k == "SHLVL", k == "_", k == "OLDPWD":
		default:
			t.Errorf("unexpected variable %s in the executor's environment", k)
		}
	}
	for k := range o.tokens {
		if k != "PATH" {
			if _, leaked := vars[k]; leaked {
				t.Errorf("the orchestrator's %s reached the executor", k)
			}
		}
	}
	for _, s := range o.secrets() {
		if strings.Contains(env, s) {
			t.Error("one of the orchestrator's credentials is in the executor's environment")
		}
	}
	if task, err := os.ReadFile(vars["AG_TASK_FILE"]); err != nil || !strings.Contains(string(task), t1.ID) {
		t.Errorf("the task file: %v %q", err, task)
	}

	// The credential is a narrow agent credential for exactly this run's repository.
	c1, c2 := w.srv.client(tok1, "agent-1"), w.srv.client(tok2, "agent-2")
	p1, err := c1.Whoami(context.Background())
	if err != nil {
		t.Fatalf("the agent's credential does not work during its run: %v", err)
	}
	if p1.Kind != "agent" || p1.Identity != "agent:crashy-a" {
		t.Errorf("principal = %+v", p1)
	}
	if len(p1.Scope.Repos) != 1 || p1.Scope.Repos[0] != w.repo {
		t.Errorf("scope repos = %v", p1.Scope.Repos)
	}
	if len(p1.Scope.Operations) == 0 {
		t.Error("the credential is not restricted to a list of operations")
	}
	for _, op := range p1.Scope.Operations {
		if config.ForbiddenAgentOps[op] {
			t.Errorf("the agent credential carries %q", op)
		}
	}
	if left := time.Until(p1.ExpiresAt()); left <= 0 || left > time.Hour {
		t.Errorf("the credential expires in %s, want within the hour", left)
	}
	if p2, err := c2.Whoami(context.Background()); err != nil || p2.CredentialID == p1.CredentialID {
		t.Errorf("second session credential: %+v %v", p2, err)
	}

	// Session 1's credential cannot be used as session 2's run, nor to do what no agent may.
	for name, req := range map[string]struct {
		method, path string
		body         any
		headers      []string
	}{
		"borrowing another run":        {"POST", "/v1/repos/" + w.repo + "/workspaces", map[string]any{"name": "stolen", "stream": "main"}, []string{"ag-run", run2}},
		"heartbeat for another run":    {"POST", "/v1/repos/" + w.repo + "/runs/" + run2 + "/heartbeat", map[string]any{}, nil},
		"completing another run":       {"POST", "/v1/repos/" + w.repo + "/runs/" + run2 + "/complete", map[string]any{"status": "completed", "summary": "x"}, nil},
		"minting credentials":          {"POST", "/v1/credentials", map[string]any{"subject": "agent:crashy-a", "ttl_secs": 600}, nil},
		"changing the policy":          {"PUT", "/v1/repos/" + w.repo + "/policy", map[string]any{"stream": "main", "document": "{}"}, nil},
		"granting itself a role":       {"POST", "/v1/roles", map[string]any{"subject": "agent:crashy-a", "repo": w.repo, "role": "admin"}, nil},
		"creating a repository":        {"POST", "/v1/repos", map[string]any{"name": "evil/repo"}, nil},
		"finishing another agent task": {"POST", "/v1/repos/" + w.repo + "/tasks/" + t2.ID + "/finish", map[string]any{"state": "done"}, nil},
	} {
		code, body := w.srv.call(tok1, req.method, req.path, req.body, req.headers...)
		if code < 400 {
			t.Errorf("%s: the server answered %d %s", name, code, body)
		}
	}
	if got := w.getTask(t2.ID); got.State != agwp.TaskInProgress {
		t.Errorf("session 1 changed task 2: %+v", got)
	}
	if code, _ := w.srv.call(tok1, "GET", "/v1/repos/"+w.repo+"/tasks", nil); code != 200 {
		t.Errorf("the credential should still work for what it is for: %d", code)
	}

	// While it runs, the status API shows the session but no secret.
	st := o.status()
	if len(st.Sessions) != 2 {
		t.Fatalf("sessions = %+v", st.Sessions)
	}

	// Let both fail.
	if err := os.WriteFile(proceed, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{t1.ID, t2.ID} {
		id := id
		eventually(t, "task "+id+" to be given up", 30*time.Second, func() bool { return w.getTask(id).State == agwp.TaskCancelled })
	}
	for _, c := range []struct {
		task, run, tok string
	}{{t1.ID, run1, tok1}, {t2.ID, run2, tok2}} {
		runs := w.runsOfTask(c.task)
		if len(runs) != 1 || runs[0].RunID != c.run || runs[0].Status != "failed" {
			t.Fatalf("runs of %s = %+v", c.task, runs)
		}
		if !strings.Contains(runs[0].Summary, "3") || strings.Contains(runs[0].Summary, c.tok) {
			t.Errorf("run summary %q: want the exit code, not the token", runs[0].Summary)
		}
		if !strings.Contains(runs[0].Summary, "[redacted]") && !strings.Contains(runs[0].Summary, "REDACTED") {
			t.Logf("note: the summary has no redaction marker: %q", runs[0].Summary)
		}
	}

	// The credentials are revoked, the leases gone.
	for name, c := range map[string]*agwp.Client{"session 1": c1, "session 2": c2} {
		if _, err := c.Whoami(context.Background()); !agwp.IsCode(err, agwp.CodeUnauthenticated) {
			t.Errorf("%s: the credential still works after the session ended: %v", name, err)
		}
	}
	eventually(t, "leases to be released", 10*time.Second, func() bool { return len(w.activeLeases()) == 0 })

	// Nothing leaked into the orchestrator's own outputs.
	_, status := o.get("/status")
	_, metrics := o.get("/metrics")
	haystacks := map[string]string{"log": o.logs.String(), "status": status, "metrics": metrics}
	for name, h := range haystacks {
		for _, s := range append(o.secrets(), tok1, tok2) {
			if strings.Contains(h, s) {
				t.Errorf("a credential appears in the orchestrator's %s", name)
			}
		}
	}
	for _, ex := range []string{"crashy-a", "crashy-b"} {
		line := fmt.Sprintf(`orchestrator_sessions_finished_total{repo="acme/web",executor=%q,outcome="failed"} 1`, ex)
		if !strings.Contains(metrics, line) {
			t.Errorf("the failure of %s is not counted (want %s):\n%s", ex, line, metrics)
		}
	}
}

// A documented limit rather than a feature: the server binds a credential to
// an agent name, not to a run, so two concurrent sessions of the SAME agent can
// act as each other's run. The orchestrator cannot fix that (a credential's
// scope has no run pin); the operator gets isolation by giving concurrent
// sessions distinct agents. This test pins the observed behaviour so the
// README's statement stays true; if it fails, the server became stricter and
// the limitation can be removed from the docs.
func TestSameAgentRunsShareTheServersIdentity(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"twin"}})
	dir := t.TempDir()
	release := filepath.Join(dir, "release")
	sh := writeScript(t, "twin.sh", fmt.Sprintf(`printf '%%s' "$AG_TOKEN" > %[1]s/token-$AG_TASK.txt
printf '%%s' "$AG_RUN" > %[1]s/run-$AG_TASK.txt
while [ ! -f %[2]s ]; do sleep 0.1; done
`, dir, release))
	w.seed(map[string]string{"README.md": "x\n"})
	t1 := w.task("Twin one", "", []string{"a/**"})
	time.Sleep(5 * time.Millisecond)
	t2 := w.task("Twin two", "", []string{"b/**"})
	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      slots: 2\n      default_executor: twin\n",
		Executors: commandExecutor("twin", sh, "    slots: 2\n"),
	})
	o.waitReady()
	eventually(t, "both sessions to start", 30*time.Second, func() bool {
		return fileExists(filepath.Join(dir, "run-"+t1.ID+".txt")) && fileExists(filepath.Join(dir, "run-"+t2.ID+".txt"))
	})
	tok1, run2 := readFile(t, filepath.Join(dir, "token-"+t1.ID+".txt")), readFile(t, filepath.Join(dir, "run-"+t2.ID+".txt"))
	code, body := w.srv.call(tok1, "POST", "/v1/repos/"+w.repo+"/runs/"+run2+"/heartbeat", map[string]any{})
	if code != 200 {
		t.Fatalf("the server now refuses a run-1 credential for run 2 of the same agent (%d %s): update the security model in the README", code, body)
	}
	if err := os.WriteFile(release, nil, 0o600); err != nil {
		t.Fatal(err)
	}
}

// Killing the executor from outside (the session is lost) ends the run as
// failed, releases the lease, and the task is retried within its budget.
func TestKilledExecutorAbandonsItsRunAndTheTaskIsRetried(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"fragile"}})
	dir := t.TempDir()
	pidfile, marker := filepath.Join(dir, "pid"), filepath.Join(dir, "first-attempt-done")
	sh := writeScript(t, "fragile.sh", fmt.Sprintf(`if [ -f %[2]s ]; then exit 0; fi
touch %[2]s
echo $$ > %[1]s.tmp && mv %[1]s.tmp %[1]s
exec sleep 600
`, pidfile, marker))
	w.seed(map[string]string{"README.md": "x\n"})
	tk := w.task("Fragile work", "", []string{"lib/**"})

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      default_executor: fragile\n      attempts: 2\n      backoff: 1s\n",
		Executors: commandExecutor("fragile", sh, ""),
	})
	o.waitReady()
	eventually(t, "the executor to start", 30*time.Second, func() bool { return fileExists(pidfile) })
	pid, err := strconv.Atoi(readFile(t, pidfile))
	if err != nil {
		t.Fatal(err)
	}
	first := w.runOfTask(tk.ID)
	if first == nil || first.Status != "running" {
		t.Fatalf("run = %+v", first)
	}
	var lease *agwp.Lease
	for _, l := range w.activeLeases() {
		if strings.Contains(l.Holder, first.RunID) {
			l := l
			lease = &l
		}
	}
	if lease == nil {
		t.Fatalf("the running session holds no lease: %+v", w.activeLeases())
	}

	if err := syscall.Kill(pid, syscall.SIGKILL); err != nil {
		t.Fatal(err)
	}
	eventually(t, "the first run to be recorded as failed", 20*time.Second, func() bool {
		for _, r := range w.runsOfTask(tk.ID) {
			if r.RunID == first.RunID {
				return r.Status == "failed"
			}
		}
		return false
	})
	for _, r := range w.runsOfTask(tk.ID) {
		if r.RunID == first.RunID && !strings.Contains(strings.ToLower(r.Summary), "kill") && !strings.Contains(strings.ToLower(r.Summary), "signal") {
			t.Errorf("the failed run's summary should say the executor was killed by a signal: %q", r.Summary)
		}
	}
	for _, l := range w.activeLeases() {
		if l.ID == lease.ID {
			t.Fatalf("the lease of the abandoned run is still active: %+v", l)
		}
	}

	// The retry (a second run) succeeds and the task is done.
	eventually(t, "the retry to finish the task", 45*time.Second, func() bool { return w.getTask(tk.ID).State == agwp.TaskDone })
	runs := w.runsOfTask(tk.ID)
	if len(runs) != 2 {
		t.Fatalf("runs = %+v", runs)
	}
	eventually(t, "leases to be released", 10*time.Second, func() bool { return len(w.activeLeases()) == 0 })
}

// A run whose supervisor is gone (an orchestrator that crashed, or a session
// whose start outcome was unknown) is found by its stale heartbeat, completed
// as abandoned, and its lease is released. The orchestrator's clock is skewed
// so that the server's timestamps look old, instead of waiting a minute.
func TestOrphanedRunIsReapedAndItsLeaseReleased(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"ghost"}})
	ctx := context.Background()
	w.seed(map[string]string{"README.md": "x\n"})
	idle := writeScript(t, "idle.sh", "exit 0\n")
	clk := &skewClock{}
	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      default_executor: ghost\n",
		Executors: commandExecutor("ghost", idle, ""),
		Clock:     clk,
	})
	o.waitReady()

	// An agent run nobody supervises: started with the agent's own credential, taking a lease.
	key, err := agentkey.Generate()
	if err != nil {
		t.Fatal(err)
	}
	m, err := w.srv.admin.MintCredential(ctx, agwp.MintRequest{Subject: "agent:ghost", TTLSecs: 600, Scope: agwp.CredentialScope{Repos: []string{w.repo}}})
	if err != nil {
		t.Fatal(err)
	}
	ghost := w.srv.client(m.Token.Reveal(), "ghost")
	if _, err := ghost.RegisterAgent(ctx, w.repo, "ghost", key.PublicHex()); err != nil {
		t.Fatalf("register: %v", err)
	}
	run, err := ghost.StartRun(ctx, w.repo, "ghost", "")
	if err != nil {
		t.Fatal(err)
	}
	asRun := ghost.WithRun(run.RunID)
	if _, err := asRun.AcquireLease(ctx, w.repo, "lost/**", false, 30*time.Minute, ""); err != nil {
		t.Fatalf("lease: %v", err)
	}
	if len(w.activeLeases()) != 1 {
		t.Fatalf("leases = %+v", w.activeLeases())
	}

	// Fresh heartbeat: not stale yet, so the orchestrator leaves it alone.
	time.Sleep(2500 * time.Millisecond)
	if r := w.runOfTaskless(run.RunID); r == nil || r.Status != "running" {
		t.Fatalf("a fresh run was touched: %+v", r)
	}

	clk.Skew(10 * time.Minute) // the heartbeat is now ten minutes old for the orchestrator
	eventually(t, "the orphaned run to be completed", 30*time.Second, func() bool {
		r := w.runOfTaskless(run.RunID)
		return r != nil && r.Status == "failed"
	})
	r := w.runOfTaskless(run.RunID)
	if !strings.Contains(r.Summary, "abandoned") {
		t.Errorf("summary = %q", r.Summary)
	}
	eventually(t, "the orphan's lease to be released", 10*time.Second, func() bool { return len(w.activeLeases()) == 0 })
	if !strings.Contains(o.logs.String(), "orphaned run") {
		t.Errorf("the reaping is not logged:\n%s", o.logs.String())
	}
}

func (w *world) runOfTaskless(runID string) *agwp.Run {
	w.t.Helper()
	runs, err := w.alice.ListRuns(context.Background(), w.repo, "", false, 200)
	if err != nil {
		w.t.Fatal(err)
	}
	for i := range runs {
		if runs[i].RunID == runID {
			return &runs[i]
		}
	}
	return nil
}
