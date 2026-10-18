package execsched

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeclock"
)

// These tests run against the real ag-server (internal/testutil). The scheduler's
// own clock is fake so refresh intervals can be crossed instantly; the server's
// clock is the wall clock.

const repo = testutil.RepoName

const seedA = "package a\n\nvar X = 1\n"

type harness struct {
	t       *testing.T
	w       *testutil.World
	px      *testutil.Proxy
	s       *Scheduler
	clk     *fakeclock.Fake
	metrics *metrics.Service
	seed    string
	n       int
}

// checksPolicy requires the named checks (runnable by the trusted CI worker) and minApprovals reviews.
func checksPolicy(ciKey string, minApprovals int, checks ...string) string {
	var req []string
	for _, c := range checks {
		req = append(req, fmt.Sprintf(`{"check": %q, "run": {"argv": ["true"], "timeout_secs": 60, "memory_mb": 128}}`, c))
	}
	return fmt.Sprintf(`{"trusted_issuers": [{"identity": "service:ci-1", "public_key": %q}], "required_validations": [%s], "approvals": {"min": %d}}`,
		ciKey, strings.Join(req, ","), minApprovals)
}

func newHarness(t *testing.T, enqueue bool) *harness {
	t.Helper()
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	h := &harness{t: t, w: w, clk: fakeclock.New(time.Time{}), metrics: metrics.NewService(metrics.NewRegistry())}
	h.seed = w.Seed(map[string]string{"README.md": "hi\n", "src/a.go": seedA}) // before any policy
	// The proxy forwards to the real server and records what the scheduler asks (and can fail one request).
	h.px = testutil.NewProxy(t, w.Srv.URL)
	c := w.Srv.ClientAt(h.px.URL, w.OrchT, "orchestrator", agwp.WithClock(h.clk))
	rc := config.Repo{Name: repo, Stream: "main", ExecSched: config.ExecSched{Enqueue: &enqueue}}
	h.s = New(Options{Repo: rc, API: c, Clock: h.clk, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
		Loop: config.Loop{Reconcile: time.Minute}, Refresh: 15 * time.Second})
	return h
}

// policy installs a stream policy on main.
func (h *harness) policy(minApprovals int, checks ...string) {
	h.t.Helper()
	if err := h.w.Srv.Admin.SetPolicy(context.Background(), repo, "main", checksPolicy(h.w.CI.PublicKey(), minApprovals, checks...)); err != nil {
		h.t.Fatalf("set policy: %v", err)
	}
}

func (h *harness) pass() {
	h.t.Helper()
	if err := h.s.Reconcile(context.Background()); err != nil {
		h.t.Fatalf("reconcile: %v", err)
	}
}

// propose opens a real proposal that changes src/a.go.
func (h *harness) propose(title string) string {
	h.t.Helper()
	h.n++
	p, _ := h.w.Propose(fmt.Sprintf("ws-%d", h.n), title, testutil.Put("src/a.go", "package a // "+title+"\n"))
	return p.ID
}

func (h *harness) proposal(id string) *agwp.Proposal {
	h.t.Helper()
	d, err := h.w.Alice.GetProposal(context.Background(), repo, id)
	if err != nil {
		h.t.Fatal(err)
	}
	return &d.Proposal
}

func (h *harness) status(id string) ProposalStatus {
	h.t.Helper()
	for _, p := range h.s.Status().Open {
		if p.ID == id {
			return p
		}
	}
	h.t.Fatalf("proposal %s not in the status: %+v", id, h.s.Status().Open)
	return ProposalStatus{}
}

func (h *harness) prepares() int { return h.px.Count("POST", "/prepare") }

func codes(bs []Blocker) string {
	var out []string
	for _, b := range bs {
		out = append(out, b.Code)
	}
	return strings.Join(out, ",")
}

func TestFourStatesWithoutAnyJobs(t *testing.T) {
	h := newHarness(t, false)
	h.policy(1, "tests")
	id := h.propose("add feature")
	h.pass()
	st := h.status(id)
	if st.Mergeable != Yes || st.Validated != No || st.Approved != No || st.Integrated || st.Ready {
		t.Fatalf("states = %+v", st)
	}
	if len(st.Jobs) != 0 {
		t.Fatal("with enqueueing off no job is queued")
	}
	if got := codes(st.Blockers); got != agwp.BlockerMissingValidation+","+agwp.BlockerInsufficientApproval {
		t.Fatalf("blockers = %v", got)
	}
	if len(st.Validations) != 1 || st.Validations[0].Check != "tests" || st.Validations[0].State != "missing" {
		t.Fatalf("validations = %+v", st.Validations)
	}
	if st.CandidateTree == "" || len(st.ChangedFiles) != 1 || st.ChangedFiles[0] != "src/a.go" || st.Strategy != "fast_forward" {
		t.Fatalf("details = %+v", st)
	}
}

func TestQueuedJobsMakeValidationPendingAndAreCountedOnce(t *testing.T) {
	h := newHarness(t, true)
	h.policy(0, "tests", "lint")
	id := h.propose("x")
	h.pass()
	st := h.status(id)
	if st.Validated != Pending || len(st.Jobs) != 2 || st.Jobs[0].State != "queued" {
		t.Fatalf("status = %+v", st)
	}
	if got := h.metrics.ValidationsQueued.With(repo).Value(); got != 2 {
		t.Fatalf("queued jobs counted = %v", got)
	}
	h.clk.Advance(20 * time.Second) // unsettled -> prepared again after the refresh interval
	h.pass()
	if got := h.metrics.ValidationsQueued.With(repo).Value(); got != 2 {
		t.Fatalf("the same jobs were counted again: %v", got)
	}
	jobs, err := h.w.Alice.ListJobs(context.Background(), repo, "", 100)
	if err != nil || len(jobs) != 2 {
		t.Fatalf("the server holds %d jobs (%v); asking again must not queue more", len(jobs), err)
	}
}

func TestEnqueueCanBeDisabled(t *testing.T) {
	h := newHarness(t, false)
	h.policy(0, "tests")
	h.propose("x")
	h.pass()
	var saw bool
	for _, r := range h.px.Requests() {
		if r.Method == "POST" && strings.HasSuffix(r.Path, "/prepare") {
			var body map[string]any
			_ = json.Unmarshal([]byte(r.Body), &body)
			if body["enqueue_validations"] != false {
				t.Fatalf("prepare body = %s", r.Body)
			}
			saw = true
		}
	}
	if !saw {
		t.Fatal("no prepare call seen")
	}
	if jobs, _ := h.w.Alice.ListJobs(context.Background(), repo, "", 100); len(jobs) != 0 {
		t.Fatalf("jobs were queued although enqueueing is off: %+v", jobs)
	}
}

func TestStatesFollowAttestationsAndApprovalsThroughEvents(t *testing.T) {
	h := newHarness(t, true)
	h.policy(1, "tests")
	id := h.propose("x")
	h.pass()
	if st := h.status(id); st.Validated != Pending || st.Approved != No {
		t.Fatalf("before: %+v", st)
	}

	job := h.w.CI.Claim()
	if job == nil || job.Validation.Proposal != id {
		t.Fatalf("claimed %+v", job)
	}
	h.w.CI.Complete(job, "pass")
	h.s.OnEvent(agwp.Event{Topic: "attestation.submit"})
	h.pass()
	if st := h.status(id); st.Validated != Yes || st.Approved != No || st.Ready {
		t.Fatalf("after the attestation: %+v", st)
	}
	if _, err := h.w.Bob.ReviewProposal(context.Background(), repo, id, agwp.DecisionApprove, "fine"); err != nil {
		t.Fatal(err)
	}
	h.s.OnEvent(agwp.Event{Topic: "proposal.review", Payload: agwp.EventPayload{Target: id}})
	h.pass()
	st := h.status(id)
	if st.Validated != Yes || st.Approved != Yes || st.Mergeable != Yes || !st.Ready || st.Integrated {
		t.Fatalf("after the approval: %+v", st)
	}
	// an integrated proposal moves to the history
	h.w.Land(h.proposal(id))
	h.pass()
	if len(h.s.Status().Open) != 0 {
		t.Fatalf("open = %+v", h.s.Status().Open)
	}
	rec := h.s.Status().Integrated
	if len(rec) != 1 || rec[0].ID != id || !rec[0].Integrated || rec[0].Ready {
		t.Fatalf("recently integrated = %+v", rec)
	}
}

func TestSettledProposalsArePreparedAgainOnlyWhenTheStreamMoves(t *testing.T) {
	h := newHarness(t, true)
	id := h.propose("x") // no policy: green from the start
	h.pass()
	if st := h.status(id); !st.Ready || st.Validated != NA {
		t.Fatalf("status = %+v", st)
	}
	n := h.prepares()
	h.clk.Advance(5 * time.Minute)
	h.pass()
	h.pass()
	if h.prepares() != n {
		t.Fatalf("a settled proposal was prepared again without a reason (%d -> %d)", n, h.prepares())
	}
	// the stream moves (someone else's change lands): every open proposal must be prepared against the new head
	other, _ := h.w.Propose("ws-readme", "readme", testutil.Put("README.md", "changed\n"))
	h.w.Land(other)
	moved := h.w.Head("main")
	h.s.OnEvent(agwp.Event{Topic: "stream.integrate"})
	h.pass()
	if h.prepares() != n+1 {
		t.Fatalf("expected one more preparation, %d -> %d", n, h.prepares())
	}
	if st := h.status(id); st.StreamHead != moved {
		t.Fatalf("prepared against %q, want %q", st.StreamHead, moved)
	}
}

func TestUnsettledProposalsAreRefreshedOnATimer(t *testing.T) {
	h := newHarness(t, true)
	h.policy(1)
	h.propose("x")
	h.pass()
	n := h.prepares()
	h.clk.Advance(5 * time.Second)
	h.pass()
	if h.prepares() != n {
		t.Fatal("refreshed too early")
	}
	h.clk.Advance(11 * time.Second)
	h.pass()
	if h.prepares() != n+1 {
		t.Fatalf("expected a refresh after the interval, %d -> %d", n, h.prepares())
	}
}

// conflicting opens two proposals that change the same line differently and lands the first.
func (h *harness) conflicting() (landed, conflicted string) {
	h.t.Helper()
	p1, _ := h.w.Propose("ws-c1", "x = 2", testutil.Put("src/a.go", "package a\n\nvar X = 2\n"))
	p2, _ := h.w.Propose("ws-c2", "x = 3", testutil.Put("src/a.go", "package a\n\nvar X = 3\n"))
	h.w.Land(p1)
	return p1.ID, p2.ID
}

func TestConflictsMakeAProposalUnmergeable(t *testing.T) {
	h := newHarness(t, true)
	_, id := h.conflicting()
	h.pass()
	st := h.status(id)
	if st.Mergeable != No || len(st.Conflicts) != 1 || st.Conflicts[0].Path != "src/a.go" || st.Ready || st.Base == "" {
		t.Fatalf("status = %+v", st)
	}
}

func TestAProposalIntoAnEmptyStreamIsHandled(t *testing.T) {
	// a repository without any changeset: the very first proposal is prepared against an empty stream
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	clk := fakeclock.New(time.Time{})
	c := w.Srv.ClientAt(w.Srv.URL, w.OrchT, "orchestrator", agwp.WithClock(clk))
	enq := true
	s := New(Options{Repo: config.Repo{Name: repo, Stream: "main", ExecSched: config.ExecSched{Enqueue: &enq}}, API: c, Clock: clk, Metrics: metrics.NewService(metrics.NewRegistry()),
		Log: slog.New(slog.NewTextHandler(io.Discard, nil)), Loop: config.Loop{Reconcile: time.Minute}, Refresh: 15 * time.Second})
	p, _ := w.Propose("ws-first", "first", testutil.Put("README.md", "hi\n"))
	if err := s.Reconcile(context.Background()); err != nil {
		t.Fatalf("reconcile: %v", err)
	}
	var st ProposalStatus
	for _, o := range s.Status().Open {
		if o.ID == p.ID {
			st = o
		}
	}
	if st.ID == "" || st.Error != "" {
		t.Fatalf("status = %+v", s.Status().Open)
	}
	if st.Mergeable == Unknown {
		t.Fatalf("the proposal into an empty stream is not classified: %+v", st)
	}
}

func TestFailedValidationIsReported(t *testing.T) {
	h := newHarness(t, true)
	h.policy(0, "tests")
	id := h.propose("x")
	h.pass()
	job := h.w.CI.Claim()
	if job == nil {
		t.Fatal("no job was queued")
	}
	h.w.CI.Complete(job, "fail")
	h.s.OnEvent(agwp.Event{Topic: "attestation.submit"})
	h.pass()
	st := h.status(id)
	if st.Validated != No || st.Ready {
		t.Fatalf("status = %+v", st)
	}
	if len(st.Validations) != 1 || st.Validations[0].State != "failed" || st.Validations[0].AttestationID == "" {
		t.Fatalf("validations = %+v", st.Validations)
	}
	if !strings.Contains(codes(st.Blockers), agwp.BlockerFailedValidation) {
		t.Fatalf("blockers = %+v", st.Blockers)
	}
}

func TestPrepareFailuresAreIsolatedAndRecorded(t *testing.T) {
	h := newHarness(t, true)
	a := h.propose("a")
	b := h.propose("b")
	// The proxy answers one prepare with a 500 without involving the server; the other goes through.
	h.px.Inject(testutil.Fault{Method: "POST", Path: "/prepare", Times: 1, Status: 500, Code: "internal"})
	err := h.s.Reconcile(context.Background())
	if err == nil {
		t.Fatal("the failing preparation must be reported")
	}
	// the other proposal was still prepared
	got := 0
	for _, id := range []string{a, b} {
		if st, ok := h.s.Latest(id); ok && st.Error == "" && st.Mergeable == Yes {
			got++
		}
	}
	if got != 1 {
		t.Fatalf("exactly one proposal should be prepared: %+v", h.s.Status().Open)
	}
	var failing ProposalStatus
	for _, p := range h.s.Status().Open {
		if p.Error != "" {
			failing = p
		}
	}
	if failing.ID == "" || failing.Mergeable != Unknown || failing.Validated != Unknown || failing.Approved != Unknown {
		t.Fatalf("failing proposal = %+v", failing)
	}
	// it is retried after the refresh interval
	h.clk.Advance(20 * time.Second)
	h.pass()
	for _, p := range h.s.Status().Open {
		if p.Error != "" || p.Mergeable != Yes {
			t.Fatalf("not recovered: %+v", p)
		}
	}
}

func TestClosedProposalsDisappearWithoutBeingCountedAsIntegrated(t *testing.T) {
	h := newHarness(t, true)
	id := h.propose("x")
	h.pass()
	if _, err := h.w.Alice.CloseProposal(context.Background(), repo, id); err != nil {
		t.Fatal(err)
	}
	h.pass()
	st := h.s.Status()
	if len(st.Open) != 0 || len(st.Integrated) != 0 {
		t.Fatalf("status = %+v", st)
	}
}

func TestForgetListenersHearOfClosedAndIntegratedProposals(t *testing.T) {
	h := newHarness(t, true)
	var gone []string
	h.s.SubscribeForget(func(id string) { gone = append(gone, id) })
	closed := h.propose("closed")
	landedP, _ := h.w.Propose("ws-landed", "landed", testutil.Put("docs/landed.md", "landed\n"))
	open := h.propose("open")
	h.pass()
	if _, err := h.w.Alice.CloseProposal(context.Background(), repo, closed); err != nil {
		t.Fatal(err)
	}
	h.w.Land(landedP)
	h.pass()
	sort.Strings(gone)
	want := []string{closed, landedP.ID}
	sort.Strings(want)
	if len(gone) != 2 || gone[0] != want[0] || gone[1] != want[1] {
		t.Fatalf("forgotten = %v, want %v (and not %s)", gone, want, open)
	}
	h.pass()
	if len(gone) != 2 {
		t.Fatalf("a proposal must be forgotten once: %v", gone)
	}
}

func TestListenersSeeEveryPreparation(t *testing.T) {
	h := newHarness(t, true)
	var got []Prepared
	h.s.Subscribe(func(p Prepared) { got = append(got, p) })
	_, id := h.conflicting()
	head := h.w.Head("main")
	h.pass()
	if len(got) != 1 || got[0].Proposal.ID != id || got[0].Prep.Base == "" || len(got[0].Prep.Conflicts) != 1 || got[0].Repo != repo || got[0].StreamHead != head {
		t.Fatalf("got = %+v", got)
	}
}

func TestOpenProposalPathsFeedTheTaskScheduler(t *testing.T) {
	h := newHarness(t, true)
	id := h.propose("x")
	h.pass()
	paths := h.s.OpenProposalPaths(repo)
	if len(paths[id]) != 1 || paths[id][0] != "src/a.go" {
		t.Fatalf("paths = %+v", paths)
	}
}

func TestReadsOnlyAndNeverIntegrates(t *testing.T) {
	h := newHarness(t, true)
	h.propose("x")
	h.pass()
	if len(h.px.Requests()) == 0 {
		t.Fatal("the scheduler made no request")
	}
	for _, r := range h.px.Requests() {
		if strings.Contains(r.Path, "integrate") {
			t.Fatalf("unexpected call %s %s", r.Method, r.Path)
		}
		if r.Method != "GET" && !(r.Method == "POST" && strings.HasSuffix(r.Path, "/prepare")) {
			t.Fatalf("unexpected mutation %s %s", r.Method, r.Path)
		}
	}
	for _, p := range func() []agwp.Proposal {
		ps, _ := h.w.Alice.ListProposals(context.Background(), repo, agwp.ProposalFilter{})
		return ps
	}() {
		if p.Title == "x" && p.State != agwp.ProposalOpen {
			t.Fatalf("the proposal must still be open: %+v", p)
		}
	}
}

func TestStatusReportsTheValidationAndIntegrationQueues(t *testing.T) {
	h := newHarness(t, true)
	h.policy(0, "tests")
	h.propose("x")
	h.pass()
	st := h.s.Status()
	if len(st.Jobs) != 1 || st.Jobs[0].State != agwp.JobQueued || st.StreamHead != h.seed || st.StreamVersion == 0 || st.LastPass.IsZero() {
		t.Fatalf("status = %+v", st)
	}
}

// The queues in /status are display data: not being able to read them does not fail the pass, but the status
// says why they are empty instead of looking like an empty queue.
func TestUnreadableQueuesAreReportedNotHidden(t *testing.T) {
	h := newHarness(t, false)
	h.propose("x")
	// The proxy answers the two listings with a 500 without involving the server; preparing is untouched.
	h.px.Inject(testutil.Fault{Method: "GET", Path: "/jobs", Times: -1, Status: 500, Code: "internal"})
	h.px.Inject(testutil.Fault{Method: "GET", Path: "/queue", Times: -1, Status: 500, Code: "internal"})
	h.pass()
	st := h.s.Status()
	if !strings.Contains(st.ListError, "validation jobs") || !strings.Contains(st.ListError, "integration queue") {
		t.Fatalf("queue_error = %q, status = %+v", st.ListError, st)
	}
	if len(st.Open) != 1 || st.Open[0].Mergeable != Yes {
		t.Fatalf("the proposals must still be tracked: %+v", st.Open)
	}
	h.px.ClearFaults()
	h.pass()
	if got := h.s.Status().ListError; got != "" {
		t.Fatalf("queue_error = %q after the queues became readable", got)
	}
}

func TestClassifyTable(t *testing.T) {
	p := agwp.Proposal{ID: "p", Stream: "main", Head: "h"}
	tests := []struct {
		name                     string
		prep                     agwp.Preparation
		mergeable, valid, approv string
		ready                    bool
	}{
		{"all green", agwp.Preparation{CandidateTree: "t"}, Yes, NA, Yes, true},
		{"passed validation", agwp.Preparation{CandidateTree: "t", Validations: []agwp.ValidationStatus{{State: "passed"}}}, Yes, Yes, Yes, true},
		{"pending job", agwp.Preparation{CandidateTree: "t", Validations: []agwp.ValidationStatus{{State: "pending", JobID: "j"}},
			Blockers: []agwp.Blocker{{Code: agwp.BlockerMissingValidation}}}, Yes, Pending, Yes, false},
		{"missing without a job", agwp.Preparation{CandidateTree: "t", Validations: []agwp.ValidationStatus{{State: "missing"}},
			Blockers: []agwp.Blocker{{Code: agwp.BlockerMissingValidation}}}, Yes, No, Yes, false},
		{"pending plus missing", agwp.Preparation{CandidateTree: "t", Validations: []agwp.ValidationStatus{{State: "pending"}, {State: "missing"}},
			Blockers: []agwp.Blocker{{Code: agwp.BlockerMissingValidation}}}, Yes, No, Yes, false},
		{"failed wins over passed", agwp.Preparation{CandidateTree: "t", Validations: []agwp.ValidationStatus{{State: "passed"}, {State: "failed"}}}, Yes, No, Yes, true},
		{"changes requested", agwp.Preparation{CandidateTree: "t", Blockers: []agwp.Blocker{{Code: agwp.BlockerChangesRequested}}}, Yes, NA, No, false},
		{"missing provenance blocks readiness but is not an approval", agwp.Preparation{CandidateTree: "t", Blockers: []agwp.Blocker{{Code: agwp.BlockerMissingProvenance}}}, Yes, NA, Yes, false},
		{"actor not allowed is about the orchestrator", agwp.Preparation{CandidateTree: "t", Blockers: []agwp.Blocker{{Code: agwp.BlockerActorNotAllowed}}}, Yes, NA, Yes, true},
		{"conflicts", agwp.Preparation{Conflicts: []agwp.Conflict{{Path: "x"}}}, No, NA, Yes, false},
		{"no candidate", agwp.Preparation{}, Unknown, NA, Yes, false},
	}
	for _, tt := range tests {
		got := classify(p, &tt.prep)
		if got.Mergeable != tt.mergeable || got.Validated != tt.valid || got.Approved != tt.approv || got.Ready != tt.ready {
			t.Errorf("%s: mergeable=%s validated=%s approved=%s ready=%v; want %s %s %s %v", tt.name, got.Mergeable, got.Validated, got.Approved, got.Ready,
				tt.mergeable, tt.valid, tt.approv, tt.ready)
		}
	}
}

func TestRunLoopPreparesNewProposalsWhenKicked(t *testing.T) {
	h := newHarness(t, true)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.s.Run(ctx)
	deadline := time.Now().Add(5 * time.Second)
	for h.s.Runs() < 1 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	id := h.propose("late")
	h.s.OnEvent(agwp.Event{Topic: "proposal.create", Payload: agwp.EventPayload{Target: id}})
	for time.Now().Before(deadline) {
		if _, ok := h.s.Latest(id); ok {
			return
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatal("the loop did not prepare the new proposal")
}
