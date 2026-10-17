package scheduler

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

type harness struct {
	t       *testing.T
	fake    *fakeagwp.Server
	orch    *agwp.Client
	mgr     *session.Manager
	sched   *Scheduler
	clk     *clock.Fake
	metrics *metrics.Service
	dir     string
}

type paths map[string][]string

func (p paths) OpenProposalPaths(string) map[string][]string { return p }

func scripted(name string, files map[string]string) config.Executor {
	return config.Executor{Name: name, Type: "scripted", Agent: name, Slots: 4, Timeout: 20 * time.Second, Grace: time.Second,
		Beat: 50 * time.Millisecond, Cred: 10 * time.Minute, Ops: append([]string(nil), config.DefaultAgentOps...), Output: 64 << 10,
		Script: config.Scripted{Files: files, Message: "change", Title: "Change " + name, Model: "demo"}}
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
	h := &harness{t: t, fake: fakeagwp.New(), clk: clock.NewFake(time.Time{}), metrics: metrics.NewService(metrics.NewRegistry()), dir: t.TempDir()}
	t.Cleanup(h.fake.Close)
	h.fake.SetClock(h.clk.Now)
	h.fake.AddRepo(repo)
	h.fake.SetHead(repo, "main", h.fake.Commit(repo, nil, map[string]string{"README.md": "hi\n"}, "user:root", "seed"))
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin"})
	var err error
	h.orch, err = agwp.New(h.fake.URL, secret.New(tok), agwp.WithIdentityLabel("orchestrator"))
	if err != nil {
		t.Fatal(err)
	}
	keys, err := agentkey.OpenStore(filepath.Join(h.dir, "keys"))
	if err != nil {
		t.Fatal(err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	h.mgr, err = session.NewManager(session.Options{Orch: h.orch, ServerURL: h.fake.URL, Keys: keys, WorkRoot: filepath.Join(h.dir, "work"),
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

// addTask creates a task and moves the fake clock on, so creation times differ
// and "oldest first" is deterministic.
func (h *harness) addTask(title string, scope []string, deps ...string) string {
	id := h.fake.AddTask(repo, title, scope, deps...)
	h.clk.Advance(time.Second)
	return id
}

func (h *harness) reconcile() {
	h.t.Helper()
	if err := h.sched.Reconcile(context.Background()); err != nil {
		h.t.Fatalf("reconcile: %v", err)
	}
}

func (h *harness) waitIdle() {
	h.t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for len(h.mgr.Active()) != 0 {
		if time.Now().After(deadline) {
			h.t.Fatalf("sessions still running: %+v", h.mgr.Active())
		}
		time.Sleep(2 * time.Millisecond)
	}
}

func (h *harness) state(id string) string { return h.fake.Task(repo, id).State }

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
	deadline := time.Now().Add(10 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			h.t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(2 * time.Millisecond)
	}
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
	h := newHarness(t, nil, nil, scripted("dev", map[string]string{"x.txt": "x"}))
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
	h.fake.IntegrateProposal(repo, st.Waiting[0].Proposals[0])
	h.reconcile()
	if h.state(a) != agwp.TaskDone {
		t.Fatalf("A = %s, want done once its proposal landed", h.state(a))
	}
	h.reconcile()
	if len(h.mgr.Active()) != 1 || h.mgr.Active()[0].TaskID != b {
		t.Fatalf("B should start after A is done: %+v", h.mgr.Active())
	}
	h.waitIdle()
}

func TestAClosedUnmergedProposalCancelsTheTask(t *testing.T) {
	h := newHarness(t, nil, nil, scripted("dev", map[string]string{"x.txt": "x"}))
	a := h.addTask("A", nil)
	h.reconcile()
	h.waitIdle()
	h.reconcile()
	w := h.sched.Status().Waiting
	if len(w) != 1 {
		t.Fatalf("waiting = %+v", w)
	}
	h.fake.CloseProposalFixture(repo, w[0].Proposals[0])
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
	var held bool
	for _, l := range h.fake.Leases(repo) {
		if l.Scope == "docs/**" && l.ReleasedAtMs == 0 && l.TaskID == t1 {
			held = true
		}
	}
	if !held {
		t.Fatalf("leases = %+v", h.fake.Leases(repo))
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
	t1 := h.addTask("docs", []string{"docs/**"})
	t2 := h.addTask("api docs", []string{"docs/api/**"})
	h.reconcile()
	if len(h.mgr.Active()) != 2 {
		t.Fatalf("allow must start both, got %+v", h.mgr.Active())
	}
	h.waitIdle()
	_ = t1
	_ = t2
	t3 := h.addTask("locked", []string{"locked/**"})
	h.fake.AddLease(repo, "user:alice", "locked/**", true, time.Hour)
	h.reconcile()
	if d, ok := h.deferral(t3); !ok || d.Reason != ReasonExclusiveLease {
		t.Fatalf("an exclusive foreign lease defers even under allow: %+v %v", d, ok)
	}
	if h.state(t3) != agwp.TaskOpen {
		t.Fatal("t3 must not have started")
	}
}

func TestOpenProposalsOnTheScopeDeferNewWork(t *testing.T) {
	src := paths{"prop_other": {"docs/index.md"}}
	h := newHarness(t, nil, src, command(t, "bot", `true`))
	h.fake.AddProposal(repo, "user:alice", "main", h.fake.Commit(repo, nil, map[string]string{"a": "b"}, "user:alice", "x"), "alice's change", "")
	// the fake assigns its own proposal id: rewrite the path map to it
	id := h.fake.Proposals(repo)[0].ID
	delete(src, "prop_other")
	src[id] = []string{"docs/index.md"}
	tk := h.addTask("docs", []string{"docs/**"})
	h.reconcile()
	d, ok := h.deferral(tk)
	if !ok || d.Reason != ReasonOpenProposal {
		t.Fatalf("deferral = %+v %v", d, ok)
	}
	h.fake.CloseProposalFixture(repo, id)
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
	runs := h.fake.Runs(repo)
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

func TestOrphanedRunsAreCompletedAndTheirTasksAdopted(t *testing.T) {
	h := newHarness(t, nil, nil, command(t, "bot", `true`))
	// A previous orchestrator process started a run for our agent and died.
	cl := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "agent:bot"})
	agent, _ := agwp.New(h.fake.URL, secret.New(cl))
	k, _ := agentkey.Generate()
	if _, err := agent.RegisterAgent(context.Background(), repo, "bot", k.PublicHex()); err != nil {
		t.Fatal(err)
	}
	// store the same key so the manager accepts the registered agent
	keysDir := filepath.Join(h.dir, "keys")
	if err := k.Save(filepath.Join(keysDir, "bot.json")); err != nil {
		t.Fatal(err)
	}
	tk := h.addTask("interrupted", nil)
	run, err := h.orch.StartRun(context.Background(), repo, "bot", tk)
	if err != nil {
		t.Fatal(err)
	}
	h.clk.Advance(10 * time.Minute) // its heartbeats stopped long ago

	h.reconcile()
	runs := h.fake.Runs(repo)
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
	cl := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "agent:bot"})
	agent, _ := agwp.New(h.fake.URL, secret.New(cl))
	k, _ := agentkey.Generate()
	_, _ = agent.RegisterAgent(context.Background(), repo, "bot", k.PublicHex())
	if _, err := h.orch.StartRun(context.Background(), repo, "bot", ""); err != nil {
		t.Fatal(err)
	}
	h.clk.Advance(20 * time.Second) // within 3 heartbeat intervals / a minute
	h.reconcile()
	if h.fake.Runs(repo)[0].Status != "running" {
		t.Fatal("a recently heartbeating run must not be reaped")
	}
}

func TestRestartRediscoversWorkThatIsWaitingToLand(t *testing.T) {
	h := newHarness(t, nil, nil, scripted("dev", map[string]string{"x.txt": "x"}))
	tk := h.addTask("A", nil)
	h.reconcile()
	h.waitIdle()
	pid := h.fake.Proposals(repo)[0].ID

	// A new scheduler instance (the orchestrator restarted) knows nothing.
	restarted, err := New(Options{Repo: config.Repo{Name: repo, Stream: "main", Schedule: config.Schedule{Slots: 2, Placement: "avoid", Lease: "soft", LeaseTTL: time.Minute, Attempts: 1, Backoff: time.Second, Default: "dev"}},
		Execs: []config.Executor{scripted("dev", map[string]string{"x.txt": "x"})}, API: h.orch, Sessions: h.mgr, Clock: h.clk, Metrics: h.metrics,
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
	if w := restarted.Status().Waiting; len(w) != 1 || w[0].Task != tk || w[0].Proposals[0] != pid {
		t.Fatalf("waiting = %+v", w)
	}
	h.fake.IntegrateProposal(repo, pid)
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
	// a person cancels the task (the fake lets us act as the unowned/any principal via the owner path)
	if err := h.mgr.FinishTaskAsOwner(context.Background(), repo, "bot", tk, "cancelled"); err != nil {
		t.Fatal(err)
	}
	h.reconcile()
	h.waitIdle()
	runs := h.fake.Runs(repo)
	if len(runs) != 1 || runs[0].Status != "failed" || !strings.Contains(runs[0].Summary, "cancelled: task cancelled by someone else") {
		t.Fatalf("runs = %+v", runs)
	}
}

func TestRoutesPickTheExecutorAndUnroutedTasksAreLeftAlone(t *testing.T) {
	docs := scripted("docs-bot", map[string]string{"docs/x.md": "x"})
	code := scripted("code-bot", map[string]string{"src/x.go": "x"})
	h := newHarness(t, func(r *config.Repo) {
		r.Schedule.Routes = []config.Route{{Executor: "docs-bot", Title: "^docs:"}, {Executor: "code-bot", Scope: "src/**"}}
	}, nil, docs, code)
	d := h.addTask("docs: write the guide", nil)
	c := h.addTask("refactor", []string{"src/lib/**"})
	other := h.addTask("unrelated", []string{"ops/**"})
	h.reconcile()
	h.waitIdle()
	owner := func(id string) string { return h.fake.Task(repo, id).Owner }
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
	waiting := a // whichever of the two is not running is deferred (their creation times are equal)
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
	h.fake.FailNext("task.list", 1, 500, "internal", false, 0)
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
