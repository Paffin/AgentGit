package advisor

import (
	"context"
	"io"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/execsched"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// These tests run against the real ag-server (internal/testutil): the conflicts
// the advisor looks at are the ones the server's merge engine reports for real
// proposals, and the three versions of a file are fetched from the real server.

const repo = testutil.RepoName

// The default scenario: a base, a change that lands on the stream ("ours") and a proposal made from the
// same base ("theirs") that now conflicts with it.
var (
	baseFiles   = map[string]string{"a.txt": "1\n2\n3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n)\n", "img.bin": "x"}
	oursFiles   = map[string]string{"a.txt": "1\nO2\n3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n"}
	theirsFiles = map[string]string{"a.txt": "1\n2\nT3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n\t\"io\"\n)\n"}
)

type harness struct {
	t       *testing.T
	w       *testutil.World
	px      *testutil.Proxy
	client  *agwp.Client // the advisor's credential, through the proxy
	comp    *Component
	metrics *metrics.Service
	base    string // head of main before "ours" landed
	ours    string // head of main now
	theirs  string // head of the conflicting proposal
	prop    agwp.Proposal
}

func newHarness(t *testing.T, mut func(*config.Repo)) *harness {
	return newScenario(t, mut, baseFiles, oursFiles, theirsFiles, nil, nil)
}

// newScenario seeds base, opens "theirs" and "ours" from it, lands "ours" and leaves "theirs" conflicting.
// A nil file value in ours/theirs deletes the file.
func newScenario(t *testing.T, mut func(*config.Repo), base, ours, theirs map[string]string, oursDel, theirsDel []string) *harness {
	t.Helper()
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	h := &harness{t: t, w: w, metrics: metrics.NewService(metrics.NewRegistry())}
	h.base = w.Seed(base)
	changes := func(files map[string]string, del []string) []agwp.FileChange {
		var ch []agwp.FileChange
		for p, c := range files {
			ch = append(ch, testutil.Put(p, c))
		}
		for _, p := range del {
			ch = append(ch, testutil.Delete(p))
		}
		return ch
	}
	// both are made from the same base, before either lands
	theirsP, _ := w.Propose("ws-theirs", "change", changes(theirs, theirsDel)...)
	oursP, _ := w.Propose("ws-ours", "stream change", changes(ours, oursDel)...)
	w.Land(oursP)
	h.ours = w.Head("main")
	d, err := w.Alice.GetProposal(context.Background(), repo, theirsP.ID)
	if err != nil {
		t.Fatal(err)
	}
	h.prop, h.theirs = d.Proposal, d.Proposal.Head

	h.px = testutil.NewProxy(t, w.Srv.URL)
	h.client = w.Srv.ClientAt(h.px.URL, w.Mint("service:advisor", nil), "advisor")
	rc := config.Repo{Name: repo, Stream: "main", Advisor: config.Advisor{Inline: 4096, MaxFile: 1 << 20, MaxPaths: 20}}
	if mut != nil {
		mut(&rc)
	}
	h.comp = New(Options{Repo: rc, API: h.client, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	return h
}

// prepared asks the real server what integrating the proposal would do now (the
// merge base and the conflicts are its answer) and keeps only the conflicts at
// the given paths. No path: no conflicts.
func (h *harness) prepared(paths ...string) execsched.Prepared {
	h.t.Helper()
	prep, err := h.w.Orch.Prepare(context.Background(), repo, h.prop.ID, false)
	if err != nil {
		h.t.Fatal(err)
	}
	keep := map[string]bool{}
	for _, p := range paths {
		keep[p] = true
	}
	var cs []agwp.Conflict
	for _, c := range prep.Conflicts {
		if keep[c.Path] {
			cs = append(cs, c)
		}
	}
	if len(cs) != len(paths) {
		h.t.Fatalf("the server reported conflicts %+v, wanted exactly %v", prep.Conflicts, paths)
	}
	prep.Conflicts = cs
	return execsched.Prepared{Repo: repo, Proposal: h.prop, StreamHead: h.w.Head("main"), Prep: *prep}
}

func (h *harness) reviews() []agwp.Review {
	h.t.Helper()
	d, err := h.w.Alice.GetProposal(context.Background(), repo, h.prop.ID)
	if err != nil {
		h.t.Fatal(err)
	}
	return d.Reviews
}

func TestPostsOneCommentWithAnInlineSuggestion(t *testing.T) {
	h := newHarness(t, nil)
	p := h.prepared("a.txt", "lib.go")
	if p.Prep.Base != h.base {
		t.Fatalf("merge base = %s, want %s", p.Prep.Base, h.base)
	}
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	rs := h.reviews()
	if len(rs) != 1 {
		t.Fatalf("reviews = %+v", rs)
	}
	r := rs[0]
	if r.Decision != agwp.DecisionComment || r.Reviewer != "service:advisor" || r.Head != h.theirs {
		t.Fatalf("review = %+v", r)
	}
	for _, want := range []string{
		"advice only", "Nothing below is applied automatically", "### `a.txt` — neighbouring edits", "### `lib.go` — both sides extended the same list",
		"1\nO2\nT3\n4\n",        // the suggested a.txt
		"\t\"io\"\n\t\"os\"\n)", // lib.go imports united and sorted
		"sha256", "ag-advisor/1 digest=", "`resolutions`", "confidence medium", "confidence high",
	} {
		if !strings.Contains(r.Body, want) {
			t.Errorf("comment lacks %q:\n%s", want, r.Body)
		}
	}
	// The three versions were fetched from the right revisions.
	fetched := map[string]int{}
	for _, rq := range h.px.Requests() {
		if rq.Method == "GET" && strings.Contains(rq.Path, "/files/") {
			fetched[rq.Query]++
		}
	}
	for _, rev := range []string{h.base, h.ours, h.theirs} {
		if fetched["rev="+rev] != 2 {
			t.Errorf("revision %s fetched %d times, want once per conflicting file; all: %v", rev[:8], fetched["rev="+rev], fetched)
		}
	}
	if h.metrics.ReviewsPosted.With(repo, "advisor", "conflict").Value() != 1 || h.metrics.AdvisorSuggest.With(repo, ClassAdjacent).Value() != 1 || h.metrics.AdvisorSuggest.With(repo, ClassUnion).Value() != 1 {
		t.Fatal("metrics not recorded")
	}
}

func TestOnlyEverComments(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
	posted := 0
	for _, rq := range h.px.Requests() {
		if rq.Method == "POST" && strings.HasSuffix(rq.Path, "/reviews") {
			posted++
			if !strings.Contains(rq.Body, `"decision":"comment"`) {
				t.Fatalf("a review that is not a comment was sent: %s", rq.Body)
			}
		}
		if strings.Contains(rq.Body, "approve") || strings.Contains(rq.Body, "request_changes") {
			t.Fatalf("approval-related decision in %s %s", rq.Path, rq.Body)
		}
		if rq.Method != "GET" && !strings.HasSuffix(rq.Path, "/reviews") {
			t.Fatalf("unexpected mutation %s %s", rq.Method, rq.Path)
		}
	}
	if posted != 1 {
		t.Fatalf("%d comments posted", posted)
	}
}

func TestTheSameAdviceIsPostedOnce(t *testing.T) {
	h := newHarness(t, nil)
	p := h.prepared("a.txt")
	for i := 0; i < 3; i++ {
		if err := h.comp.Process(context.Background(), p); err != nil {
			t.Fatal(err)
		}
	}
	if len(h.reviews()) != 1 {
		t.Fatalf("reviews = %d", len(h.reviews()))
	}
	// a restarted advisor (no memory) recognises its earlier comment by the marker
	again := New(Options{Repo: config.Repo{Name: repo, Stream: "main", Advisor: config.Advisor{Inline: 4096, MaxFile: 1 << 20, MaxPaths: 20}}, API: h.client,
		Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	if err := again.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 1 {
		t.Fatalf("a restarted advisor repeated itself: %d reviews", len(h.reviews()))
	}
	if rec, ok := again.Suggestions(h.prop.ID); !ok || !rec.Posted || rec.ReviewID == 0 {
		t.Fatalf("record = %+v %v", rec, ok)
	}
}

func TestNewConflictsGetNewAdvice(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
	if err := h.comp.Process(context.Background(), h.prepared("a.txt", "lib.go")); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 2 {
		t.Fatalf("reviews = %d, want 2", len(h.reviews()))
	}
}

func TestNoConflictsMeansNoAdviceAndForgetsOldRecords(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
	if _, ok := h.comp.Suggestions(h.prop.ID); !ok {
		t.Fatal("record missing")
	}
	h.comp.OnPrepared(h.prepared()) // the conflict went away
	if _, ok := h.comp.Suggestions(h.prop.ID); ok {
		t.Fatal("stale advice kept")
	}
	if err := h.comp.Process(context.Background(), h.prepared()); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 1 {
		t.Fatal("no new comment expected")
	}
}

func TestForgetDropsEverythingAboutAProposal(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
	h.comp.OnPrepared(h.prepared("a.txt")) // queued again
	h.comp.Forget(h.prop.ID)
	if _, ok := h.comp.Suggestions(h.prop.ID); ok {
		t.Fatal("advice kept for a proposal that is gone")
	}
	if st := h.comp.Status(); len(st.Records) != 0 {
		t.Fatalf("status = %+v", st)
	}
	h.comp.Forget("nonexistent") // harmless
	// If the same id came back (it cannot, but the memory must not suppress advice), advice would be given again.
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
}

func TestBinaryAndHugeFilesAreNotAnalysed(t *testing.T) {
	big := strings.Repeat("line\n", 1000)
	h := newScenario(t, func(r *config.Repo) { r.Advisor.MaxFile = 2048 },
		map[string]string{"big.txt": "x\n", "bin.dat": "a\x00b"},
		map[string]string{"big.txt": big + "o\n", "bin.dat": "a\x00c"},
		map[string]string{"big.txt": big + "t\n", "bin.dat": "a\x00d"}, nil, nil)
	if err := h.comp.Process(context.Background(), h.prepared("big.txt", "bin.dat")); err != nil {
		t.Fatal(err)
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if len(rec.Suggestions) != 2 || rec.Suggestions[0].Class != ClassBinary || rec.Suggestions[1].Class != ClassBinary {
		t.Fatalf("suggestions = %+v", rec.Suggestions)
	}
}

func TestMissingVersionsMeanDeleteOrBothAdded(t *testing.T) {
	h := newScenario(t, nil,
		map[string]string{"gone.txt": "keep\n"},
		map[string]string{"new.txt": "o\n"},
		map[string]string{"gone.txt": "changed\n", "new.txt": "t\n"},
		[]string{"gone.txt"}, nil) // ours deleted gone.txt and added new.txt; theirs edited gone.txt and added another new.txt
	if err := h.comp.Process(context.Background(), h.prepared("gone.txt")); err != nil {
		t.Fatal(err)
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if rec.Suggestions[0].Class != ClassDeleteEdit || rec.Suggestions[0].Complete {
		t.Fatalf("suggestion = %+v", rec.Suggestions[0])
	}
	if !strings.Contains(h.reviews()[0].Body, "deleted on one side, changed on the other") {
		t.Fatal("the comment does not explain the delete/modify conflict")
	}
	// "both added": the file is missing from the base
	if err := h.comp.Process(context.Background(), h.prepared("new.txt")); err != nil {
		t.Fatal(err)
	}
	rec, _ = h.comp.Suggestions(h.prop.ID)
	if rec.Suggestions[0].Class != ClassBothAdded {
		t.Fatalf("suggestion = %+v", rec.Suggestions[0])
	}
}

func TestLimitsTheNumberOfFilesAnalysed(t *testing.T) {
	h := newScenario(t, func(r *config.Repo) { r.Advisor.MaxPaths = 2 },
		map[string]string{"a.txt": "1\n2\n3\n4\n", "b.txt": "1\n2\n3\n4\n", "c.txt": "1\n2\n3\n4\n"},
		map[string]string{"a.txt": "1\nO2\n3\n4\n", "b.txt": "1\nO2\n3\n4\n", "c.txt": "1\nO2\n3\n4\n"},
		map[string]string{"a.txt": "1\nT2\n3\n4\n", "b.txt": "1\nT2\n3\n4\n", "c.txt": "1\nT2\n3\n4\n"}, nil, nil)
	p := h.prepared("a.txt", "b.txt", "c.txt")
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if len(rec.Suggestions) != 3 || !strings.Contains(rec.Suggestions[2].Path, "1 more conflicting file") {
		t.Fatalf("suggestions = %+v", rec.Suggestions)
	}
}

func TestClosedProposalsGetNoComment(t *testing.T) {
	h := newHarness(t, nil)
	p := h.prepared("a.txt")
	if _, err := h.w.Alice.CloseProposal(context.Background(), repo, h.prop.ID); err != nil {
		t.Fatal(err)
	}
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 0 {
		t.Fatal("a closed proposal must not be commented on")
	}
}

func TestFetchFailuresAreReportedAndRetried(t *testing.T) {
	h := newHarness(t, nil)
	// The proxy fails one file request without involving the server.
	h.px.Inject(testutil.Fault{Method: "GET", Path: "/files/", Times: 1, Status: 500, Code: "internal"})
	p := h.prepared("a.txt")
	if err := h.comp.Process(context.Background(), p); err == nil {
		t.Fatal("expected the fetch failure")
	}
	if len(h.reviews()) != 0 {
		t.Fatal("nothing may be posted on a failure")
	}
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatalf("retry: %v", err)
	}
	if len(h.reviews()) != 1 {
		t.Fatal("the retry should have posted")
	}
}

func TestPostFailuresAreRecordedAndRetried(t *testing.T) {
	h := newHarness(t, nil)
	h.px.Inject(testutil.Fault{Method: "POST", Path: "/reviews", Times: 1, Status: 500, Code: "internal"})
	p := h.prepared("a.txt")
	if err := h.comp.Process(context.Background(), p); err == nil {
		t.Fatal("expected the post failure")
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if rec.Posted || rec.Error == "" {
		t.Fatalf("record = %+v", rec)
	}
	if err := h.comp.Process(context.Background(), p); err != nil || len(h.reviews()) != 1 {
		t.Fatalf("err=%v reviews=%d", err, len(h.reviews()))
	}
}

// watcher stands for whoever learns from the server that the comment exists - a person with the proposal page
// open, a test polling the proposal - and asks the advisor's own API for the full advice the moment it does.
// It runs inside ReviewProposal, after the server has the comment and before the advisor has been told it is done.
type watcher struct {
	API
	comp *Component
	got  chan watched
}

type watched struct {
	rec    Record
	ok     bool
	status Status
}

func (w *watcher) ReviewProposal(ctx context.Context, repo, id, decision, body string) (*agwp.Review, error) {
	rv, err := w.API.ReviewProposal(ctx, repo, id, decision, body)
	if err == nil {
		go func() {
			rec, ok := w.comp.Suggestions(id)
			w.got <- watched{rec: rec, ok: ok, status: w.comp.Status()}
		}()
		time.Sleep(100 * time.Millisecond) // the head start a loaded machine gives the watcher over the advisor's own goroutine
	}
	return rv, err
}

func TestAdviceIsReadableAsSoonAsItsCommentIsVisible(t *testing.T) {
	h := newHarness(t, nil)
	w := &watcher{API: h.client, comp: h.comp, got: make(chan watched, 1)}
	h.comp.o.API = w
	if err := h.comp.Process(context.Background(), h.prepared("a.txt")); err != nil {
		t.Fatal(err)
	}
	select {
	case seen := <-w.got:
		if !seen.ok || !seen.rec.Posted || seen.rec.ReviewID == 0 || len(seen.rec.Suggestions) != 1 {
			t.Fatalf("the comment was visible but the advice was not available: ok=%v record=%+v", seen.ok, seen.rec)
		}
		if len(seen.status.Records) != 1 || !seen.status.Records[0].Posted {
			t.Fatalf("status = %+v", seen.status)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the watcher never got an answer")
	}
}

func TestRunLoopWorksThroughQueuedPreparations(t *testing.T) {
	h := newHarness(t, nil)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.comp.Run(ctx)
	h.comp.OnPrepared(h.prepared("a.txt"))
	deadline := time.Now().Add(5 * time.Second)
	for len(h.reviews()) == 0 && time.Now().Before(deadline) {
		time.Sleep(2 * time.Millisecond)
	}
	if len(h.reviews()) != 1 {
		t.Fatal("the loop did not post")
	}
	st := h.comp.Status() // no waiting for the record: it is there as soon as the comment is
	if st.Advisor != "rules/1" || len(st.Records) != 1 || !st.Records[0].Posted || st.Records[0].Files[0] != "a.txt" || st.Records[0].Complete != 1 {
		t.Fatalf("status = %+v", st)
	}
}

func TestEndToEndThroughTheExecutionScheduler(t *testing.T) {
	h := newHarness(t, nil)
	sched := execsched.New(execsched.Options{Repo: config.Repo{Name: repo, Stream: "main"}, API: h.w.Orch, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	sched.Subscribe(h.comp.OnPrepared)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.comp.Run(ctx)
	if err := sched.Reconcile(ctx); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for len(h.reviews()) == 0 && time.Now().Before(deadline) {
		time.Sleep(2 * time.Millisecond)
	}
	rs := h.reviews()
	if len(rs) != 1 || !strings.Contains(rs[0].Body, "1\nO2\nT3\n4\n") {
		t.Fatalf("reviews = %+v", rs)
	}
}

func TestRenderFencesAroundBackticksAndCapsTheSize(t *testing.T) {
	s := Suggestion{Path: "README.md", Class: ClassWhitespace, Complete: true, Confidence: High, Advisor: "rules/1", SHA256: strings.Repeat("a", 64),
		Resolved: []byte("```go\nfmt.Println()\n```\n"), Regions: []Region{{Class: ClassWhitespace, Resolution: "ours", Why: "because", Confidence: High, BaseStart: 1, BaseEnd: 1}}}
	out := Render("rules/1", "abcdef0123456789", "fedcba9876543210", "main", []Suggestion{s}, 4096, "dddd")
	if !strings.Contains(out, "````\n```go\nfmt.Println()\n```\n````\n") {
		t.Fatalf("the inner fence was not protected:\n%s", out)
	}
	if !strings.HasSuffix(strings.TrimSpace(out), "-->") || !strings.Contains(out, "<!-- ag-advisor/1 digest=dddd -->") {
		t.Fatalf("marker missing:\n%s", out)
	}
	many := make([]Suggestion, 0, 200)
	for i := 0; i < 200; i++ {
		many = append(many, Suggestion{Path: "f.txt", Class: ClassNeedsPerson, Notes: []string{strings.Repeat("n", 1000)}})
	}
	big := Render("rules/1", "a", "b", "main", many, 4096, "x")
	if len(big) > maxComment+2000 || !strings.Contains(big, "omitted to keep this comment small") {
		t.Fatalf("comment length %d", len(big))
	}
	// too large to inline: only the digest is mentioned
	s.Resolved = []byte(strings.Repeat("x\n", 5000))
	out = Render("rules/1", "a", "b", "main", []Suggestion{s}, 100, "dddd")
	if strings.Contains(out, "xx") || !strings.Contains(out, "too large to inline") {
		t.Fatalf("large suggestion rendering wrong:\n%s", out)
	}
}

func TestRecordsAreBounded(t *testing.T) {
	h := newHarness(t, nil)
	h.comp.o.Remember = 3
	for i := 0; i < 6; i++ {
		h.comp.remember(&Record{Proposal: string(rune('a' + i))})
	}
	if len(h.comp.Status().Records) != 3 {
		t.Fatalf("records = %d", len(h.comp.Status().Records))
	}
	if _, ok := h.comp.Suggestions("a"); ok {
		t.Fatal("oldest record not evicted")
	}
}
