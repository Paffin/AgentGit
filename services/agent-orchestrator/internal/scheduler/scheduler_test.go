package scheduler

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeclock"
)

// These tests run against the real ag-server (internal/testutil). Agents are real
// programs: shell scripts, or the test-only agent program that writes files, signs
// provenance and opens a proposal. The scheduler's clock is a fake that starts at
// the wall clock, so backoffs and "stale" heartbeats can be crossed instantly.

const repo = testutil.RepoName

type harness struct {
	t       *testing.T
	w       *testutil.World
	px      *testutil.Proxy
	orch    *agwp.Client
	mgr     *session.Manager
	sched   *Scheduler
	clk     *fakeclock.Fake
	metrics *metrics.Service
	dir     string
}

type paths map[string][]string

func (p paths) OpenProposalPaths(string) map[string][]string { return p }

// agent is an executor that runs the test-only agent program: it writes files, signs provenance and opens a proposal.
func agent(t *testing.T, name string, files map[string]string) config.Executor {
	t.Helper()
	b, err := json.Marshal(files)
	if err != nil {
		t.Fatal(err)
	}
	return config.Executor{Name: name, Type: "command", Agent: name, Slots: 4, Timeout: 20 * time.Second, Grace: time.Second,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Command: []string{testutil.BuildTestAgent(t)}, Pass: []string{"PATH"},
		Env: map[string]string{"TESTAGENT_FILES": string(b), "TESTAGENT_MESSAGE": "change", "TESTAGENT_TITLE": "Change " + name, "TESTAGENT_MODEL": "test-agent"}}
}

func command(t *testing.T, name, script string) config.Executor {
	path := filepath.Join(t.TempDir(), name+".sh")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+script), 0o700); err != nil {
		t.Fatal(err)
	}
	return config.Executor{Name: name, Type: "command", Agent: name, Slots: 4, Timeout: 20 * time.Second, Grace: 500 * time.Millisecond,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Command: []string{"/bin/sh", path}, Pass: []string{"PATH"}}
}

func newHarness(t *testing.T, repoCfg func(*config.Repo), pathSrc PathSource, execs ...config.Executor) *harness {
	t.Helper()
	var names []string
	for _, e := range execs {
		names = append(names, e.Agent)
	}
	w := testutil.NewWorld(t, testutil.WorldOptions{Agents: names})
	h := &harness{t: t, w: w, clk: fakeclock.New(time.Now()), metrics: metrics.NewService(metrics.NewRegistry()), dir: t.TempDir()}
	w.Seed(map[string]string{"README.md": "hi\n"})
	// The proxy forwards to the real server, records what the scheduler asks and can fail one request.
	h.px = testutil.NewProxy(t, w.Srv.URL)
	h.orch = w.Srv.ClientAt(h.px.URL, w.OrchT, "orchestrator")
	keys, err := agentkey.OpenStore(filepath.Join(h.dir, "keys"))
	if err != nil {
		t.Fatal(err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	h.mgr, err = session.NewManager(session.Options{Orch: h.orch, ServerURL: w.Srv.URL, Keys: keys, WorkRoot: filepath.Join(h.dir, "work"),
		Metrics: h.metrics, Log: log, ParentEnv: func(k string) string { return os.Getenv(k) }, OnDone: func(string) { h.sched.Kick() }})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.mgr.Close)
	rc := config.Repo{Name: repo, Stream: "main", Schedule: config.Schedule{Slots: 2, Placement: "avoid", Lease: "soft", LeaseTTL: 10 * time.Minute, Attempts: 1, Backoff: 30 * time.Second}}
	if len(execs) == 1 {
		rc.Schedule.Default = execs[0].Name
	}
	if repoCfg != nil {
		repoCfg(&rc)
	}
	h.sched, err = New(Options{Repo: rc, Execs: execs, API: h.orch, Sessions: h.mgr, Paths: pathSrc, Clock: h.clk, Metrics: h.metrics, Log: log,
		Loop: config.Loop{Reconcile: time.Minute, Debounce: 0}})
	if err != nil {
		t.Fatal(err)
	}
	return h
}

// addTask files a real task. A few real milliseconds pass so creation times
// differ and "oldest first" is deterministic.
func (h *harness) addTask(title string, scope []string, deps ...string) string {
	tk := h.w.Task(title, "", scope, deps...)
	time.Sleep(5 * time.Millisecond)
	return tk.ID
}

func (h *harness) reconcile() {
	h.t.Helper()
	if err := h.sched.Reconcile(context.Background()); err != nil {
		h.t.Fatalf("reconcile: %v", err)
	}
}

func (h *harness) waitIdle() {
	h.t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for len(h.mgr.Active()) != 0 {
		if time.Now().After(deadline) {
			h.t.Fatalf("sessions still running: %+v", h.mgr.Active())
		}
		time.Sleep(2 * time.Millisecond)
	}
}

func (h *harness) task(id string) *agwp.Task { return h.w.GetTask(id) }

func (h *harness) state(id string) string { return h.task(id).State }

// runs lists every run, oldest first.
func (h *harness) runs() []agwp.Run {
	h.t.Helper()
	rs, err := h.w.Alice.ListRuns(context.Background(), repo, "", false, 200)
	if err != nil {
		h.t.Fatal(err)
	}
	sort.Slice(rs, func(i, j int) bool {
		if rs[i].StartedAtMs != rs[j].StartedAtMs {
			return rs[i].StartedAtMs < rs[j].StartedAtMs
		}
		return rs[i].RunID < rs[j].RunID
	})
	return rs
}

func (h *harness) deferral(id string) (Deferral, bool) {
	for _, d := range h.sched.Status().Deferred {
		if d.Task == id {
			return d, true
		}
	}
	return Deferral{}, false
}

func (h *harness) eventually(what string, cond func() bool) {
	h.t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			h.t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// proposalOf is the one proposal a task's agent opened.
func (h *harness) proposalOf(task string) agwp.Proposal {
	h.t.Helper()
	ps := h.w.ProposalsOfTask(task)
	if len(ps) != 1 {
		h.t.Fatalf("proposals of task %s = %+v", task, ps)
	}
	return ps[0]
}

func TestReadyTasksStartOldestFirstWithinTheRepoLimit(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `sleep 0.4`))
	var ids []string
	for i := 0; i < 3; i++ {
		ids = append(ids, h.addTask(fmt.Sprint("task ", i), nil))
	}
	h.reconcile()
	if len(h.mgr.Active()) != 2 {
		t.Fatalf("sessions = %d, want the repo limit of 2", len(h.mgr.Active()))
	}
	got := map[string]bool{}
	for _, i := range h.mgr.Active() {
		got[i.TaskID] = true
	}
	if !got[ids[0]] || !got[ids[1]] || got[ids[2]] {
		t.Fatalf("the two oldest tasks must start first, got %v", got)
	}
	if d, ok := h.deferral(ids[2]); !ok || d.Reason != ReasonRepoBusy {
		t.Fatalf("the third task should be deferred as repo_busy, got %+v %v", d, ok)
	}
	// when a slot frees up, the next pass starts the third
	h.eventually("a free slot", func() bool { return len(h.mgr.Active()) < 2 })
	h.reconcile()
	h.eventually("the third task to start or finish", func() bool { return h.state(ids[2]) != "open" })
	h.waitIdle()
	for _, id := range ids {
		if h.state(id) != agwp.TaskDone {
			t.Fatalf("task %s is %s", id, h.state(id))
		}
	}
	if h.metrics.TasksDeferred.With(repo, ReasonRepoBusy).Value() < 1 {
		t.Fatal("deferral not counted")
	}
}

func TestDependenciesAreRespectedAndWorkWaitsToLand(t *testing.T) {
	h := newHarness(t, nil, nil, agent(t, "dev", map[string]string{"x.txt": "x"}))
	a := h.addTask("A", []string{"api/**"})
	b := h.addTask("B depends on A", []string{"api/**"}, a)

	h.reconcile()
	if len(h.mgr.Active()) != 1 || h.mgr.Active()[0].TaskID != a {
		t.Fatalf("only A may start: %+v", h.mgr.Active())
	}
	h.waitIdle()
	// A's session ended with an open proposal: the task waits for it to land and B stays blocked.
	h.reconcile()
	if h.state(a) != agwp.TaskInProgress || h.state(b) != agwp.TaskOpen || len(h.mgr.Active()) != 0 {
		t.Fatalf("A=%s B=%s", h.state(a), h.state(b))
	}
	st := h.sched.Status()
	if len(st.Waiting) != 1 || st.Waiting[0].Task != a || len(st.Waiting[0].Proposals) != 1 {
		t.Fatalf("status = %+v", st)
	}
	// A human integrates the proposal; the next pass finishes A as its owner and unblocks B.
	pa := h.proposalOf(a)
	h.w.Approve(pa.ID) // the default policy wants one approval for proposals of agents
	h.w.Land(&pa)
	starts := h.px.Count("POST", "/runs")
	h.reconcile()
	if h.state(a) != agwp.TaskDone {
		t.Fatalf("A = %s, want done once its proposal landed", h.state(a))
	}
	// A task finished in this very pass must not be offered for adoption in the same pass
	// (the task list was read before it was finished).
	if n := h.px.Count("POST", "/runs"); n != starts {
		t.Fatalf("the pass that finished A also tried to start a session (%d start requests, was %d)", n, starts)
	}
	if d, ok := h.deferral(a); ok {
		t.Fatalf("a finished task is reported as deferred: %+v", d)
	}
	h.reconcile()
	if len(h.mgr.Active()) != 1 || h.mgr.Active()[0].TaskID != b {
		t.Fatalf("B should start after A is done: %+v", h.mgr.Active())
	}
	h.waitIdle()
}

func TestAClosedUnmergedProposalCancelsTheTask(t *testing.T) {
	h := newHarness(t, nil, nil, agent(t, "dev", map[string]string{"x.txt": "x"}))
	a := h.addTask("A", nil)
	h.reconcile()
	h.waitIdle()
	h.reconcile()
	w := h.sched.Status().Waiting
	if len(w) != 1 {
		t.Fatalf("waiting = %+v", w)
	}
	if _, err := h.w.Carol.CloseProposal(context.Background(), repo, w[0].Proposals[0]); err != nil {
		t.Fatal(err)
	}
	h.reconcile()
	if h.state(a) != agwp.TaskCancelled {
		t.Fatalf("A = %s", h.state(a))
	}
	if len(h.sched.Status().Waiting) != 0 {
		t.Fatal("a resolved task must not stay in the waiting list")
	}
}

func TestOverlappingScopesAreDeferredWhileTheFirstTaskHoldsTheLease(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Schedule.Slots = 3 }, nil, command(t, "bot", `sleep 0.5`))
	t1 := h.addTask("docs", []string{"docs/**"})
	t2 := h.addTask("api docs", []string{"docs/api/**"})
	t3 := h.addTask("source", []string{"src/**"})
	h.reconcile()
	active := map[string]bool{}
	for _, i := range h.mgr.Active() {
		active[i.TaskID] = true
	}
	if !active[t1] || !active[t3] || active[t2] {
		t.Fatalf("active = %v: t1 and t3 run, t2 overlaps t1", active)
	}
	d, ok := h.deferral(t2)
	if !ok || d.Reason != ReasonRunningTask || !strings.Contains(d.Detail, "docs/**") {
		t.Fatalf("t2 deferral = %+v", d)
	}
	// the soft lease of the first task is visible on the server
	leases, err := h.w.Alice.ListLeases(context.Background(), repo, true)
	if err != nil {
		t.Fatal(err)
	}
	var held bool
	for _, l := range leases {
		if l.Scope == "docs/**" && l.ReleasedAtMs == 0 && l.TaskID == t1 {
			held = true
		}
	}
	if !held {
		t.Fatalf("leases = %+v", leases)
	}
	h.eventually("t1 to finish and its session to be gone", func() bool {
		for _, i := range h.mgr.Active() {
			if i.TaskID == t1 {
				return false
			}
		}
		return h.state(t1) == agwp.TaskDone
	})
	h.reconcile()
	h.eventually("t2 to start and finish", func() bool { return h.state(t2) == agwp.TaskDone })
	h.waitIdle()
}

func TestAllowPolicyStartsOverlappingTasksButRespectsExclusiveLeases(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Schedule.Slots = 3; r.Schedule.Placement = "allow" }, nil, command(t, "bot", `sleep 0.3`))
	h.addTask("docs", []string{"docs/**"})
	h.addTask("api docs", []string{"docs/api/**"})
	h.reconcile()
	if len(h.mgr.Active()) != 2 {
		t.Fatalf("allow must start both, got %+v", h.mgr.Active())
	}
	h.waitIdle()
	t3 := h.addTask("locked", []string{"locked/**"})
	if _, err := h.w.Alice.AcquireLease(context.Background(), repo, "locked/**", true, time.Hour, ""); err != nil {
		t.Fatal(err)
	}
	h.reconcile()
	if d, ok := h.deferral(t3); !ok || d.Reason != ReasonExclusiveLease {
		t.Fatalf("an exclusive foreign lease defers even under allow: %+v %v", d, ok)
	}
	if h.state(t3) != agwp.TaskOpen {
		t.Fatal("t3 must not have started")
	}
}

func TestOpenProposalsOnTheScopeDeferNewWork(t *testing.T) {
	src := paths{}
	h := newHarness(t, nil, src, command(t, "bot", `true`))
	p, _ := h.w.Propose("ws-alice", "alice's change", testutil.Put("docs/index.md", "alice\n"))
	src[p.ID] = []string{"docs/index.md"} // what the execution scheduler reports for the open proposal
	tk := h.addTask("docs", []string{"docs/**"})
	h.reconcile()
	d, ok := h.deferral(tk)
	if !ok || d.Reason != ReasonOpenProposal {
		t.Fatalf("deferral = %+v %v", d, ok)
	}
	if _, err := h.w.Alice.CloseProposal(context.Background(), repo, p.ID); err != nil {
		t.Fatal(err)
	}
	delete(src, p.ID)
	h.reconcile()
	h.eventually("the task to run", func() bool { return h.state(tk) == agwp.TaskDone })
}

func TestFailedAttemptsAreRetriedWithBackoffThenCancelled(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Schedule.Attempts = 2; r.Schedule.Backoff = time.Minute }, nil, command(t, "bot", `echo nope >&2; exit 1`))
	tk := h.addTask("flaky", nil)
	h.reconcile()
	h.waitIdle()
	h.eventually("the retry to be recorded", func() bool { return len(h.sched.Status().Retrying) == 1 })
	if h.state(tk) != agwp.TaskInProgress {
		t.Fatalf("task = %s: a task with attempts left stays in progress", h.state(tk))
	}
	// still inside the backoff: nothing starts
	h.clk.Advance(30 * time.Second)
	h.reconcile()
	if len(h.mgr.Active()) != 0 {
		t.Fatal("started during the backoff")
	}
	if d, ok := h.deferral(tk); !ok || d.Reason != ReasonRetryBackoff {
		t.Fatalf("deferral = %+v %v", d, ok)
	}
	// after the backoff the same agent re-claims the task for attempt 2
	h.clk.Advance(31 * time.Second)
	h.reconcile()
	h.waitIdle()
	h.eventually("the task to be cancelled after the last attempt", func() bool { return h.state(tk) == agwp.TaskCancelled })
	runs := h.runs()
	if len(runs) != 2 || runs[0].Status != "failed" || runs[1].Status != "failed" {
		t.Fatalf("runs = %+v", runs)
	}
	if !strings.Contains(runs[0].Summary, "attempt 1") && !strings.Contains(runs[1].Summary, "attempt 1") {
		t.Fatalf("summaries = %q / %q", runs[0].Summary, runs[1].Summary)
	}
}

func TestTempFailDoesNotSpendTheAttemptBudget(t *testing.T) {
	counter := filepath.Join(t.TempDir(), "n")
	script := fmt.Sprintf(`n=$(cat %[1]s 2>/dev/null || echo 0); n=$((n+1)); echo $n > %[1]s; if [ $n -lt 3 ]; then exit 75; fi; exit 0`, counter)
	h := newHarness(t, func(r *config.Repo) { r.Schedule.Attempts = 1; r.Schedule.Backoff = time.Second }, nil, command(t, "bot", script))
	tk := h.addTask("temp", nil)
	for i := 0; i < 3; i++ {
		h.reconcile()
		h.waitIdle()
		h.clk.Advance(5 * time.Second)
	}
	h.eventually("done after two temporary failures", func() bool { return h.state(tk) == agwp.TaskDone })
}

// registerOrphan plays a previous orchestrator process: it registers the agent with a key kept in the key store,
// then starts a run for it that is never heartbeated or completed.
func (h *harness) registerOrphan(agentName, task string) *agwp.Run {
	h.t.Helper()
	ag := h.w.Srv.Client(h.w.Mint("agent:"+agentName, nil), "agent")
	k, err := agentkey.Generate()
	if err != nil {
		h.t.Fatal(err)
	}
	if _, err := ag.RegisterAgent(context.Background(), repo, agentName, k.PublicHex()); err != nil {
		h.t.Fatal(err)
	}
	// store the same key so the manager accepts the registered agent
	if err := k.Save(filepath.Join(h.dir, "keys", agentName+".json")); err != nil {
		h.t.Fatal(err)
	}
	run, err := h.orch.StartRun(context.Background(), repo, agentName, task)
	if err != nil {
		h.t.Fatal(err)
	}
	return run
}

func TestOrphanedRunsAreCompletedAndTheirTasksAdopted(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `true`))
	tk := h.addTask("interrupted", nil)
	run := h.registerOrphan("bot", tk)
	h.clk.Advance(10 * time.Minute) // its heartbeats stopped long ago

	h.reconcile()
	runs := h.runs()
	if runs[0].RunID != run.RunID || runs[0].Status != "failed" || !strings.HasPrefix(runs[0].Summary, "abandoned:") {
		t.Fatalf("the orphan must be completed as abandoned: %+v", runs[0])
	}
	// next pass adopts the in-progress task and finishes it
	h.reconcile()
	h.waitIdle()
	h.eventually("the adopted task to finish", func() bool { return h.state(tk) == agwp.TaskDone })
}

func TestFreshRunsOfOurAgentsAreNotReaped(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `true`))
	h.registerOrphan("bot", "")
	h.clk.Advance(20 * time.Second) // within 3 heartbeat intervals / a minute
	h.reconcile()
	if h.runs()[0].Status != "running" {
		t.Fatal("a recently heartbeating run must not be reaped")
	}
}

func TestRestartRediscoversWorkThatIsWaitingToLand(t *testing.T) {
	h := newHarness(t, nil, nil, agent(t, "dev", map[string]string{"x.txt": "x"}))
	tk := h.addTask("A", nil)
	h.reconcile()
	h.waitIdle()
	pa := h.proposalOf(tk)

	// A new scheduler instance (the orchestrator restarted) knows nothing.
	restarted, err := New(Options{Repo: config.Repo{Name: repo, Stream: "main", Schedule: config.Schedule{Slots: 2, Placement: "avoid", Lease: "soft", LeaseTTL: time.Minute, Attempts: 1, Backoff: time.Second, Default: "dev"}},
		Execs: []config.Executor{agent(t, "dev", map[string]string{"x.txt": "x"})}, API: h.orch, Sessions: h.mgr, Clock: h.clk, Metrics: h.metrics,
		Log: slog.New(slog.NewTextHandler(io.Discard, nil)), Loop: config.Loop{Reconcile: time.Minute}})
	if err != nil {
		t.Fatal(err)
	}
	if err := restarted.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(h.mgr.Active()) != 0 {
		t.Fatal("work that already produced a proposal must not be redone")
	}
	if w := restarted.Status().Waiting; len(w) != 1 || w[0].Task != tk || w[0].Proposals[0] != pa.ID {
		t.Fatalf("waiting = %+v", w)
	}
	h.w.Approve(pa.ID) // the default policy wants one approval for proposals of agents
	h.w.Land(&pa)
	if err := restarted.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if h.state(tk) != agwp.TaskDone {
		t.Fatalf("task = %s", h.state(tk))
	}
}

func TestATaskCancelledByAPersonStopsItsSession(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `sleep 30`))
	tk := h.addTask("long", nil)
	h.reconcile()
	if len(h.mgr.Active()) != 1 {
		t.Fatal("not started")
	}
	// someone other than the owner cancels the task
	if err := h.mgr.FinishTask(context.Background(), repo, tk, "cancelled"); err != nil {
		t.Fatal(err)
	}
	h.reconcile()
	h.waitIdle()
	runs := h.runs()
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.Contains(runs[0].Summary, "cancelled: task cancelled by someone else") {
		t.Fatalf("runs = %+v", runs)
	}
}

func TestRoutesPickTheExecutorAndUnroutedTasksAreLeftAlone(t *testing.T) {
	docs := agent(t, "docs-bot", map[string]string{"docs/x.md": "x"})
	code := agent(t, "code-bot", map[string]string{"src/x.go": "x"})
	h := newHarness(t, func(r *config.Repo) {
		r.Schedule.Routes = []config.Route{{Executor: "docs-bot", Title: "^docs:"}, {Executor: "code-bot", Scope: "src/**"}}
	}, nil, docs, code)
	d := h.addTask("docs: write the guide", nil)
	c := h.addTask("refactor", []string{"src/lib/**"})
	other := h.addTask("unrelated", []string{"ops/**"})
	h.reconcile()
	h.waitIdle()
	owner := func(id string) string { return h.task(id).Owner }
	if owner(d) != "agent:docs-bot" || owner(c) != "agent:code-bot" {
		t.Fatalf("owners: %q %q", owner(d), owner(c))
	}
	if h.state(other) != agwp.TaskOpen || owner(other) != "" {
		t.Fatal("a task no route matches must be left to others")
	}
}

func TestExecutorSlotsAreSharedAcrossTasks(t *testing.T) {
	ex := command(t, "bot", `sleep 0.4`)
	ex.Slots = 1
	h := newHarness(t, func(r *config.Repo) { r.Schedule.Slots = 3 }, nil, ex)
	a := h.addTask("a", nil)
	b := h.addTask("b", nil)
	h.reconcile()
	if len(h.mgr.Active()) != 1 {
		t.Fatalf("active = %d", len(h.mgr.Active()))
	}
	waiting := a // whichever of the two is not running is deferred
	if h.mgr.Active()[0].TaskID == a {
		waiting = b
	}
	if d, ok := h.deferral(waiting); !ok || d.Reason != ReasonExecutorBusy {
		t.Fatalf("deferral of the waiting task = %+v %v", d, ok)
	}
	h.waitIdle()
	h.reconcile()
	h.waitIdle()
	h.eventually("both done", func() bool { return h.state(a) == agwp.TaskDone && h.state(b) == agwp.TaskDone })
}

func TestRunLoopReactsToKicks(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `true`))
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { h.sched.Run(ctx); close(done) }()
	h.eventually("the first pass", func() bool { return h.sched.Runs() >= 1 })
	tk := h.addTask("late arrival", nil)
	h.sched.Kick()
	h.eventually("the task to be handled by the loop", func() bool { return h.state(tk) == agwp.TaskDone })
	cancel()
	<-done
	if st := h.sched.Status(); st.LastPass.IsZero() || st.LastError != "" || st.Repo != repo {
		t.Fatalf("status = %+v", st)
	}
}

func TestServerErrorsSurfaceInStatusAndDoNotCrash(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `true`))
	// The proxy answers one task listing with a 500 without involving the server.
	h.px.Inject(testutil.Fault{Method: "GET", Path: "/tasks", Times: 1, Status: 500, Code: "internal"})
	if err := h.sched.Reconcile(context.Background()); err == nil {
		t.Fatal("expected the listing failure")
	}
	if st := h.sched.Status(); !strings.Contains(st.LastError, "list tasks") {
		t.Fatalf("status.LastError = %q", st.LastError)
	}
	h.reconcile() // recovers
	if h.sched.Status().LastError != "" {
		t.Fatal("error not cleared")
	}
}

func TestNewRejectsBrokenRoutes(t *testing.T) {
	_, err := New(Options{Repo: config.Repo{Name: repo, Schedule: config.Schedule{Routes: []config.Route{{Executor: "ghost", Title: "x"}}}}})
	if err == nil {
		t.Fatal("unknown executor accepted")
	}
	_, err = New(Options{Repo: config.Repo{Name: repo, Schedule: config.Schedule{Default: "ghost"}}})
	if err == nil {
		t.Fatal("unknown default accepted")
	}
	_, err = New(Options{Repo: config.Repo{Name: repo, Schedule: config.Schedule{Routes: []config.Route{{Executor: "a", Title: "("}}}}, Execs: []config.Executor{{Name: "a"}}})
	if err == nil {
		t.Fatal("bad regexp accepted")
	}
}
