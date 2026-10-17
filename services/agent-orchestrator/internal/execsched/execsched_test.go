package execsched

import (
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

type harness struct {
	t       *testing.T
	fake    *fakeagwp.Server
	s       *Scheduler
	clk     *clock.Fake
	metrics *metrics.Service
	seed    string
}

func newHarness(t *testing.T, enqueue bool) *harness {
	t.Helper()
	h := &harness{t: t, fake: fakeagwp.New(), clk: clock.NewFake(time.Time{}), metrics: metrics.NewService(metrics.NewRegistry())}
	t.Cleanup(h.fake.Close)
	h.fake.SetClock(h.clk.Now)
	h.fake.AddRepo(repo)
	h.seed = h.fake.Commit(repo, nil, map[string]string{"README.md": "hi\n", "src/a.go": "package a\n"}, "user:root", "seed")
	h.fake.SetHead(repo, "main", h.seed)
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "writer"})
	c, err := agwp.New(h.fake.URL, secret.New(tok), agwp.WithClock(h.clk))
	if err != nil {
		t.Fatal(err)
	}
	rc := config.Repo{Name: repo, Stream: "main", ExecSched: config.ExecSched{Enqueue: &enqueue}}
	h.s = New(Options{Repo: rc, API: c, Clock: h.clk, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
		Loop: config.Loop{Reconcile: time.Minute}, Refresh: 15 * time.Second})
	return h
}

func (h *harness) pass() {
	h.t.Helper()
	if err := h.s.Reconcile(context.Background()); err != nil {
		h.t.Fatalf("reconcile: %v", err)
	}
}

// propose adds an open proposal that changes src/a.go.
func (h *harness) propose(title string) string {
	head := h.fake.Commit(repo, []string{h.seed}, map[string]string{"README.md": "hi\n", "src/a.go": "package a // " + title + "\n"}, "agent:bot#run_1", title)
	return h.fake.AddProposal(repo, "agent:bot#run_1", "main", head, title, "")
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

func (h *harness) prepares() int { return h.fake.CountRequests("integration.prepare") }

func TestFourStatesWithoutAnyJobs(t *testing.T) {
	h := newHarness(t, true)
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{RequiredChecks: []string{"tests"}, MinApprovals: 1})
	id := h.propose("add feature")
	h.pass()
	st := h.status(id)
	if st.Mergeable != Yes || st.Validated != No || st.Approved != No || st.Integrated || st.Ready {
		t.Fatalf("states = %+v", st)
	}
	if len(st.Jobs) != 0 {
		t.Fatal("the real server answers with no jobs when it cannot queue any; that must be tolerated")
	}
	codes := []string{}
	for _, b := range st.Blockers {
		codes = append(codes, b.Code)
	}
	if strings.Join(codes, ",") != "missing_validation,insufficient_approvals" {
		t.Fatalf("blockers = %v", codes)
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
	h.fake.StubJobs = true
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{RequiredChecks: []string{"tests", "lint"}})
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
}

func TestEnqueueCanBeDisabled(t *testing.T) {
	h := newHarness(t, false)
	h.propose("x")
	h.pass()
	var sawFalse bool
	for _, r := range h.fake.Requests() {
		if r.Op == "integration.prepare" {
			var body map[string]any
			_ = json.Unmarshal([]byte(r.Body), &body)
			if body["enqueue_validations"] != false {
				t.Fatalf("prepare body = %s", r.Body)
			}
			sawFalse = true
		}
	}
	if !sawFalse {
		t.Fatal("no prepare call seen")
	}
}

func TestStatesFollowAttestationsAndApprovalsThroughEvents(t *testing.T) {
	h := newHarness(t, true)
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{RequiredChecks: []string{"tests"}, MinApprovals: 1})
	id := h.propose("x")
	h.pass()
	tree := h.status(id).CandidateTree

	h.fake.Attest(repo, tree, "tests")
	h.s.OnEvent(agwp.Event{Topic: "attestation.submit"})
	h.pass()
	if st := h.status(id); st.Validated != Yes || st.Approved != No || st.Ready {
		t.Fatalf("after the attestation: %+v", st)
	}
	h.fake.Approve(repo, id, "user:bob")
	h.s.OnEvent(agwp.Event{Topic: "proposal.review", Payload: agwp.EventPayload{Target: id}})
	h.pass()
	st := h.status(id)
	if st.Validated != Yes || st.Approved != Yes || st.Mergeable != Yes || !st.Ready || st.Integrated {
		t.Fatalf("after the approval: %+v", st)
	}
	// an integrated proposal moves to the history
	h.fake.IntegrateProposal(repo, id)
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
	// the stream moves: every proposal must be prepared against the new head
	other := h.fake.Commit(repo, []string{h.seed}, map[string]string{"README.md": "changed\n", "src/a.go": "package a\n"}, "user:alice", "readme")
	h.fake.SetHead(repo, "main", other)
	h.s.OnEvent(agwp.Event{Topic: "stream.integrate"})
	h.pass()
	if h.prepares() != n+1 {
		t.Fatalf("expected one more preparation, %d -> %d", n, h.prepares())
	}
	if st := h.status(id); st.StreamHead != other {
		t.Fatalf("prepared against %q, want %q", st.StreamHead, other)
	}
}

func TestUnsettledProposalsAreRefreshedOnATimer(t *testing.T) {
	h := newHarness(t, true)
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{MinApprovals: 1})
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

func TestConflictsAndProblemsMakeAProposalUnmergeable(t *testing.T) {
	h := newHarness(t, true)
	id := h.propose("x")
	h.fake.SetPreparation(repo, id, agwp.Preparation{
		Strategy: "merge", Base: "base1", Conflicts: []agwp.Conflict{{Path: "src/a.go", Kind: "content conflict (1 region(s))"}},
	})
	h.pass()
	st := h.status(id)
	if st.Mergeable != No || len(st.Conflicts) != 1 || st.Conflicts[0].Path != "src/a.go" || st.Ready || st.Base != "base1" {
		t.Fatalf("status = %+v", st)
	}
	h.fake.SetPreparation(repo, id, agwp.Preparation{Problem: "stream 'main' has no changesets yet"})
	h.clk.Advance(time.Minute)
	h.pass()
	if st := h.status(id); st.Mergeable != No || st.Problem == "" {
		t.Fatalf("status = %+v", st)
	}
}

func TestFailedValidationIsReported(t *testing.T) {
	h := newHarness(t, true)
	id := h.propose("x")
	h.fake.SetPreparation(repo, id, agwp.Preparation{
		CandidateTree: "tree-1",
		Validations:   []agwp.ValidationStatus{{Check: "tests", State: "failed", AttestationID: "att_1"}},
		Blockers:      []agwp.Blocker{{Code: agwp.BlockerFailedValidation, Message: "check 'tests' failed"}},
	})
	h.pass()
	if st := h.status(id); st.Validated != No || st.Ready {
		t.Fatalf("status = %+v", st)
	}
}

func TestPrepareFailuresAreIsolatedAndRecorded(t *testing.T) {
	h := newHarness(t, true)
	a := h.propose("a")
	b := h.propose("b")
	h.fake.FailNext("integration.prepare", 1, 500, "internal", false, 0)
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
	h.fake.CloseProposalFixture(repo, id)
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
	closed, landed, open := h.propose("closed"), h.propose("landed"), h.propose("open")
	h.pass()
	h.fake.CloseProposalFixture(repo, closed)
	h.fake.IntegrateProposal(repo, landed)
	h.pass()
	sort.Strings(gone)
	want := []string{closed, landed}
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
	id := h.propose("x")
	h.fake.SetPreparation(repo, id, agwp.Preparation{Base: "b", CandidateTree: "t", Conflicts: []agwp.Conflict{{Path: "p", Kind: "k"}}})
	h.pass()
	if len(got) != 1 || got[0].Proposal.ID != id || got[0].Prep.Base != "b" || len(got[0].Prep.Conflicts) != 1 || got[0].Repo != repo || got[0].StreamHead != h.seed {
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
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{})
	h.propose("x")
	h.pass()
	for _, r := range h.fake.Requests() {
		if strings.Contains(r.Op, "integrate") && r.Op != "integration.prepare" {
			t.Fatalf("unexpected call %s", r.Op)
		}
		if r.Method != "GET" && r.Op != "integration.prepare" {
			t.Fatalf("unexpected mutation %s %s", r.Method, r.Path)
		}
	}
}

func TestStatusReportsTheValidationAndIntegrationQueues(t *testing.T) {
	h := newHarness(t, true)
	h.fake.StubJobs = true
	h.fake.SetPolicy(repo, "main", fakeagwp.Policy{RequiredChecks: []string{"tests"}})
	h.propose("x")
	h.pass()
	st := h.s.Status()
	if len(st.Jobs) != 1 || st.Jobs[0].State != agwp.JobQueued || st.StreamHead != h.seed || st.StreamVersion == 0 || st.LastPass.IsZero() {
		t.Fatalf("status = %+v", st)
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
