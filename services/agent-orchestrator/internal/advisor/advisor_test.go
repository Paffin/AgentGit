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
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

type harness struct {
	t       *testing.T
	fake    *fakeagwp.Server
	client  *agwp.Client
	comp    *Component
	metrics *metrics.Service
	base    string
	ours    string
	theirs  string
	prop    agwp.Proposal
}

func newHarness(t *testing.T, mut func(*config.Repo)) *harness {
	t.Helper()
	h := &harness{t: t, fake: fakeagwp.New(), metrics: metrics.NewService(metrics.NewRegistry())}
	t.Cleanup(h.fake.Close)
	h.fake.AddRepo(repo)
	h.base = h.fake.Commit(repo, nil, map[string]string{"a.txt": "1\n2\n3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n)\n", "img.bin": "x"}, "user:root", "base")
	h.ours = h.fake.Commit(repo, []string{h.base}, map[string]string{"a.txt": "1\nO2\n3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n", "img.bin": "x"}, "user:alice", "stream change")
	h.theirs = h.fake.Commit(repo, []string{h.base}, map[string]string{"a.txt": "1\n2\nT3\n4\n", "lib.go": "package lib\n\nimport (\n\t\"fmt\"\n\t\"io\"\n)\n", "img.bin": "x"}, "agent:bot#run_1", "proposal change")
	h.fake.SetHead(repo, "main", h.ours)
	pid := h.fake.AddProposal(repo, "agent:bot#run_1", "main", h.theirs, "change", "")
	h.prop = h.fake.Proposals(repo)[0]
	_ = pid
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:advisor", Role: "reviewer"})
	var err error
	h.client, err = agwp.New(h.fake.URL, secret.New(tok))
	if err != nil {
		t.Fatal(err)
	}
	rc := config.Repo{Name: repo, Stream: "main", Advisor: config.Advisor{Inline: 4096, MaxFile: 1 << 20, MaxPaths: 20}}
	if mut != nil {
		mut(&rc)
	}
	h.comp = New(Options{Repo: rc, API: h.client, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	return h
}

func (h *harness) prepared(conflicts ...agwp.Conflict) execsched.Prepared {
	return execsched.Prepared{Repo: repo, Proposal: h.prop, StreamHead: h.ours,
		Prep: agwp.Preparation{Base: h.base, Conflicts: conflicts, Strategy: "merge"}}
}

func (h *harness) reviews() []agwp.Review { return h.fake.Reviews(repo, h.prop.ID) }

func TestPostsOneCommentWithAnInlineSuggestion(t *testing.T) {
	h := newHarness(t, nil)
	p := h.prepared(agwp.Conflict{Path: "a.txt", Kind: "content conflict (1 region(s))"}, agwp.Conflict{Path: "lib.go", Kind: "content conflict (1 region(s))"})
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
	for _, rq := range h.fake.Requests() {
		if rq.Op == "tree.file" {
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
	if err := h.comp.Process(context.Background(), h.prepared(agwp.Conflict{Path: "a.txt"})); err != nil {
		t.Fatal(err)
	}
	for _, rq := range h.fake.Requests() {
		if rq.Op == "proposal.review" && !strings.Contains(rq.Body, `"decision":"comment"`) {
			t.Fatalf("a review that is not a comment was sent: %s", rq.Body)
		}
		if strings.Contains(rq.Body, "approve") || strings.Contains(rq.Body, "request_changes") {
			t.Fatalf("approval-related decision in %s %s", rq.Op, rq.Body)
		}
		if rq.Method != "GET" && rq.Op != "proposal.review" {
			t.Fatalf("unexpected mutation %s", rq.Op)
		}
	}
}

func TestTheSameAdviceIsPostedOnce(t *testing.T) {
	h := newHarness(t, nil)
	p := h.prepared(agwp.Conflict{Path: "a.txt"})
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
	if err := h.comp.Process(context.Background(), h.prepared(agwp.Conflict{Path: "a.txt"})); err != nil {
		t.Fatal(err)
	}
	if err := h.comp.Process(context.Background(), h.prepared(agwp.Conflict{Path: "a.txt"}, agwp.Conflict{Path: "lib.go"})); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 2 {
		t.Fatalf("reviews = %d, want 2", len(h.reviews()))
	}
}

func TestNoConflictsMeansNoAdviceAndForgetsOldRecords(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Process(context.Background(), h.prepared(agwp.Conflict{Path: "a.txt"})); err != nil {
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

func TestBinaryAndHugeFilesAreNotAnalysed(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Advisor.MaxFile = 2048 })
	h.fake.Commit(repo, nil, nil, "x", "unused")
	big := strings.Repeat("line\n", 1000)
	b := h.fake.Commit(repo, nil, map[string]string{"big.txt": "x\n", "bin.dat": "a\x00b"}, "u", "b")
	o := h.fake.Commit(repo, []string{b}, map[string]string{"big.txt": big + "o\n", "bin.dat": "a\x00c"}, "u", "o")
	th := h.fake.Commit(repo, []string{b}, map[string]string{"big.txt": big + "t\n", "bin.dat": "a\x00d"}, "u", "t")
	p := execsched.Prepared{Repo: repo, Proposal: agwp.Proposal{ID: h.prop.ID, Head: th, Stream: "main"}, StreamHead: o,
		Prep: agwp.Preparation{Base: b, Conflicts: []agwp.Conflict{{Path: "big.txt"}, {Path: "bin.dat"}}}}
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if len(rec.Suggestions) != 2 || rec.Suggestions[0].Class != ClassBinary || rec.Suggestions[1].Class != ClassBinary {
		t.Fatalf("suggestions = %+v", rec.Suggestions)
	}
}

func TestMissingVersionsMeanDeleteOrBothAdded(t *testing.T) {
	h := newHarness(t, nil)
	b := h.fake.Commit(repo, nil, map[string]string{"gone.txt": "keep\n"}, "u", "b")
	o := h.fake.Commit(repo, []string{b}, map[string]string{}, "u", "ours deleted it")
	th := h.fake.Commit(repo, []string{b}, map[string]string{"gone.txt": "changed\n", "new.txt": "t\n"}, "u", "theirs edited it")
	o2 := h.fake.Commit(repo, []string{b}, map[string]string{"new.txt": "o\n"}, "u", "ours added new")
	_ = o2
	p := execsched.Prepared{Repo: repo, Proposal: agwp.Proposal{ID: h.prop.ID, Head: th, Stream: "main"}, StreamHead: o,
		Prep: agwp.Preparation{Base: b, Conflicts: []agwp.Conflict{{Path: "gone.txt", Kind: "deleted on our side, modified on theirs"}}}}
	if err := h.comp.Process(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	rec, _ := h.comp.Suggestions(h.prop.ID)
	if rec.Suggestions[0].Class != ClassDeleteEdit || rec.Suggestions[0].Complete {
		t.Fatalf("suggestion = %+v", rec.Suggestions[0])
	}
	if !strings.Contains(h.reviews()[0].Body, "deleted on one side, changed on the other") {
		t.Fatal("the comment does not explain the delete/modify conflict")
	}
	// "both added": no base at all
	p2 := execsched.Prepared{Repo: repo, Proposal: agwp.Proposal{ID: h.prop.ID, Head: th, Stream: "main"}, StreamHead: o2,
		Prep: agwp.Preparation{Base: "", Conflicts: []agwp.Conflict{{Path: "new.txt", Kind: "both sides added different content"}}}}
	if err := h.comp.Process(context.Background(), p2); err != nil {
		t.Fatal(err)
	}
	rec, _ = h.comp.Suggestions(h.prop.ID)
	if rec.Suggestions[0].Class != ClassBothAdded {
		t.Fatalf("suggestion = %+v", rec.Suggestions[0])
	}
}

func TestLimitsTheNumberOfFilesAnalysed(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Advisor.MaxPaths = 2 })
	p := h.prepared(agwp.Conflict{Path: "a.txt"}, agwp.Conflict{Path: "lib.go"}, agwp.Conflict{Path: "img.bin"})
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
	h.fake.CloseProposalFixture(repo, h.prop.ID)
	if err := h.comp.Process(context.Background(), h.prepared(agwp.Conflict{Path: "a.txt"})); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews()) != 0 {
		t.Fatal("a closed proposal must not be commented on")
	}
}

func TestFetchFailuresAreReportedAndRetried(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.FailNext("tree.file", 1, 500, "internal", false, 0)
	p := h.prepared(agwp.Conflict{Path: "a.txt"})
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
	h.fake.FailNext("proposal.review", 1, 500, "internal", false, 0)
	p := h.prepared(agwp.Conflict{Path: "a.txt"})
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

func TestRunLoopWorksThroughQueuedPreparations(t *testing.T) {
	h := newHarness(t, nil)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.comp.Run(ctx)
	h.comp.OnPrepared(h.prepared(agwp.Conflict{Path: "a.txt"}))
	deadline := time.Now().Add(5 * time.Second)
	for len(h.reviews()) == 0 && time.Now().Before(deadline) {
		time.Sleep(2 * time.Millisecond)
	}
	if len(h.reviews()) != 1 {
		t.Fatal("the loop did not post")
	}
	for len(h.comp.Status().Records) == 0 && time.Now().Before(deadline) { // the record is stored just after the post
		time.Sleep(2 * time.Millisecond)
	}
	st := h.comp.Status()
	if st.Advisor != "rules/1" || len(st.Records) != 1 || !st.Records[0].Posted || st.Records[0].Files[0] != "a.txt" || st.Records[0].Complete != 1 {
		t.Fatalf("status = %+v", st)
	}
}

func TestEndToEndThroughTheExecutionScheduler(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetPreparation(repo, h.prop.ID, agwp.Preparation{Base: h.base, CandidateTree: "", Strategy: "merge",
		Conflicts: []agwp.Conflict{{Path: "a.txt", Kind: "content conflict (1 region(s))"}}})
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "writer"})
	orch, _ := agwp.New(h.fake.URL, secret.New(tok))
	sched := execsched.New(execsched.Options{Repo: config.Repo{Name: repo, Stream: "main"}, API: orch, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
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
