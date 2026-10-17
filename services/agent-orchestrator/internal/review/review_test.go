package review

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

func on(b bool) *bool { return &b }

type harness struct {
	t       *testing.T
	fake    *fakeagwp.Server
	c       *Component
	client  *agwp.Client
	metrics *metrics.Service
	seed    string
}

func newHarness(t *testing.T, emptyStream bool) *harness {
	t.Helper()
	h := &harness{t: t, fake: fakeagwp.New(), metrics: metrics.NewService(metrics.NewRegistry())}
	t.Cleanup(h.fake.Close)
	h.fake.AddRepo(repo)
	if !emptyStream {
		h.seed = h.fake.Commit(repo, nil, map[string]string{"README.md": "hello\n", "pkg/a.go": "package pkg\n"}, "user:root", "seed")
		h.fake.SetHead(repo, "main", h.seed)
	}
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:reviewer", Role: "reviewer"})
	var err error
	h.client, err = agwp.New(h.fake.URL, secret.New(tok))
	if err != nil {
		t.Fatal(err)
	}
	rc := config.Repo{Name: repo, Stream: "main", Review: config.Review{
		Enabled: on(true), MaxFiles: 40, MaxLines: 1500, Secrets: on(true), Tests: on(true),
		Protected: []string{".github/**", "**/.env"}, Generated: []string{"**/*.lock"}, Sources: []string{"**/*.go"},
	}}
	h.c, err = New(Options{Repo: rc, API: h.client, Reviewer: "service:reviewer", Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
		Loop: config.Loop{Reconcile: time.Minute}})
	if err != nil {
		t.Fatal(err)
	}
	return h
}

// propose opens a proposal whose head changes/adds the given files on top of the seed.
func (h *harness) propose(title string, files map[string]string) (id, head string) {
	snapshot := map[string]string{"README.md": "hello\n", "pkg/a.go": "package pkg\n"}
	for k, v := range files {
		snapshot[k] = v
	}
	var parents []string
	if h.seed != "" {
		parents = []string{h.seed}
	}
	head = h.fake.Commit(repo, parents, snapshot, "agent:bot#run_1", title)
	id = h.fake.AddProposal(repo, "agent:bot#run_1", "main", head, title, "")
	return id, head
}

func (h *harness) reviews(id string) []agwp.Review { return h.fake.Reviews(repo, id) }

func (h *harness) reconcile() {
	h.t.Helper()
	if err := h.c.Reconcile(context.Background()); err != nil {
		h.t.Fatal(err)
	}
}

func TestFlagsAPlantedSecretAnOversizedChangeAndProtectedPaths(t *testing.T) {
	h := newHarness(t, false)
	files := map[string]string{
		"pkg/config.go":            "package pkg\n\nvar awsKey = \"" + awsKey() + "\"\n",
		".github/workflows/ci.yml": "name: ci\n",
	}
	for i := 0; i < 45; i++ {
		files[fmt.Sprintf("docs/page%02d.md", i)] = "text\n"
	}
	id, head := h.propose("big change", files)
	h.reconcile()
	rs := h.reviews(id)
	if len(rs) != 1 {
		t.Fatalf("reviews = %+v", rs)
	}
	r := rs[0]
	if r.Decision != agwp.DecisionComment || r.Reviewer != "service:reviewer" || r.Head != head {
		t.Fatalf("review = %+v", r)
	}
	for _, want := range []string{
		"Possible secret in added lines", "`pkg/config.go:3` — AWS access key id", "Large change", "47 files", "Touches protected or sensitive paths", ".github/workflows/ci.yml",
		"neither an approval nor a request for changes", "<!-- ag-review/1 head=" + head,
	} {
		if !strings.Contains(r.Body, want) {
			t.Errorf("comment lacks %q:\n%s", want, r.Body)
		}
	}
	if strings.Contains(r.Body, awsKey()) {
		t.Fatalf("the comment repeats the secret:\n%s", r.Body)
	}
	// the secret comes first
	if strings.Index(r.Body, "Possible secret") > strings.Index(r.Body, "Large change") {
		t.Fatal("findings are not ordered by severity")
	}
	for _, kind := range []string{KindSecret, KindOversized, KindProtected} {
		if h.metrics.ReviewsPosted.With(repo, "review", kind).Value() != 1 {
			t.Errorf("metric for %s missing", kind)
		}
	}
}

func TestCleanCodeGetsNoComment(t *testing.T) {
	h := newHarness(t, false)
	id, _ := h.propose("tidy up", map[string]string{
		"pkg/handler.go":      "package pkg\n\nfunc Serve() error {\n\treturn nil\n}\n",
		"pkg/handler_test.go": "package pkg\n\nfunc TestServe(t *testing.T) {}\n",
		"docs/guide.md":       "Words.\n",
	})
	h.reconcile()
	if rs := h.reviews(id); len(rs) != 0 {
		t.Fatalf("a clean change must not be commented on: %+v", rs)
	}
	if st := h.c.Status(); len(st.Recent) != 1 || len(st.Recent[0].Findings) != 0 || st.Recent[0].Posted {
		t.Fatalf("status = %+v", st)
	}
}

func TestOneCommentPerHeadAndANewHeadGetsItsOwn(t *testing.T) {
	h := newHarness(t, false)
	id, head := h.propose("with key", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.reconcile()
	h.reconcile()
	h.reconcile()
	if len(h.reviews(id)) != 1 {
		t.Fatalf("reviews = %d", len(h.reviews(id)))
	}
	// a restarted component recognises its comment by the marker
	again, _ := New(Options{Repo: h.c.o.Repo, API: h.client, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	if err := again.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(h.reviews(id)) != 1 {
		t.Fatalf("a restart repeated the comment: %d", len(h.reviews(id)))
	}
	// the author pushes a fix that still has a different secret: new head, new comment
	head2 := h.fake.Commit(repo, []string{head}, map[string]string{"README.md": "hello\n", "pkg/a.go": "package pkg\n", "pkg/k2.go": "package pkg\n\nvar k2 = \"" + ghToken() + "\"\n"}, "agent:bot#run_1", "second")
	h.fake.MoveProposal(repo, id, head2)
	h.reconcile()
	rs := h.reviews(id)
	if len(rs) != 2 || rs[1].Head != head2 || !strings.Contains(rs[1].Body, "GitHub token") {
		t.Fatalf("reviews = %+v", rs)
	}
}

func TestFixedProposalsStopGettingComments(t *testing.T) {
	h := newHarness(t, false)
	id, head := h.propose("with key", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.reconcile()
	clean := h.fake.Commit(repo, []string{head}, map[string]string{"README.md": "hello\n", "pkg/a.go": "package pkg\n", "pkg/k.go": "package pkg\n\nvar k = os.Getenv(\"K\")\n", "pkg/k_test.go": "package pkg\n"}, "agent:bot#run_1", "fix")
	h.fake.MoveProposal(repo, id, clean)
	h.reconcile()
	// the diff is against the merge base, which is the seed: the file is still "added" but without the key
	if len(h.reviews(id)) != 1 {
		t.Fatalf("a clean head must not be commented on: %d reviews", len(h.reviews(id)))
	}
}

func TestInitialImportIntoAnEmptyStreamIsReviewed(t *testing.T) {
	h := newHarness(t, true)
	files := map[string]string{"README.md": "hello\n", "app/.env": "TOKEN=x\n", "app/main.go": "package main\n\nvar key = \"" + awsKey() + "\"\n"}
	head := h.fake.Commit(repo, nil, files, "user:alice", "initial import")
	id := h.fake.AddProposal(repo, "user:alice", "main", head, "import", "")
	h.reconcile()
	rs := h.reviews(id)
	if len(rs) != 1 || !strings.Contains(rs[0].Body, "app/main.go:3") || !strings.Contains(rs[0].Body, "app/.env") {
		t.Fatalf("reviews = %+v", rs)
	}
}

func TestReviewersNeverApproveOrRequestChanges(t *testing.T) {
	h := newHarness(t, false)
	h.propose("with key", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.reconcile()
	for _, rq := range h.fake.Requests() {
		if rq.Op == "proposal.review" && !strings.Contains(rq.Body, `"decision":"comment"`) {
			t.Fatalf("not a comment: %s", rq.Body)
		}
		if strings.Contains(rq.Body, `"approve"`) || strings.Contains(rq.Body, "request_changes") {
			t.Fatalf("approval-related decision sent: %s", rq.Body)
		}
		if rq.Method != "GET" && rq.Op != "proposal.review" && rq.Op != "tree.diff" {
			t.Fatalf("unexpected mutation %s", rq.Op)
		}
	}
}

func TestFailuresAreRetriedOnTheNextPass(t *testing.T) {
	h := newHarness(t, false)
	id, _ := h.propose("with key", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.fake.FailNext("tree.diff", 1, 500, "internal", false, 0)
	if err := h.c.Reconcile(context.Background()); err == nil {
		t.Fatal("expected the diff failure")
	}
	if len(h.reviews(id)) != 0 {
		t.Fatal("nothing may be posted on failure")
	}
	h.fake.FailNext("proposal.review", 1, 500, "internal", false, 0)
	if err := h.c.Reconcile(context.Background()); err == nil {
		t.Fatal("expected the post failure")
	}
	if st := h.c.Status(); st.Recent[0].Error == "" || st.Recent[0].Posted {
		t.Fatalf("status = %+v", st.Recent[0])
	}
	h.reconcile()
	if len(h.reviews(id)) != 1 {
		t.Fatalf("reviews = %d", len(h.reviews(id)))
	}
}

func TestClosedProposalsAreForgotten(t *testing.T) {
	h := newHarness(t, false)
	id, _ := h.propose("x", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.reconcile()
	h.fake.CloseProposalFixture(repo, id)
	h.reconcile()
	h.c.mu.Lock()
	n := len(h.c.reviewed)
	h.c.mu.Unlock()
	if n != 0 {
		t.Fatalf("reviewed set not pruned: %d", n)
	}
}

func TestRunLoopReviewsOnKick(t *testing.T) {
	h := newHarness(t, false)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.c.Run(ctx)
	deadline := time.Now().Add(5 * time.Second)
	for h.c.Runs() < 1 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	id, _ := h.propose("late", map[string]string{"pkg/k.go": "package pkg\n\nvar k = \"" + awsKey() + "\"\n"})
	h.c.OnEvent(agwp.Event{Topic: "proposal.create"})
	for len(h.reviews(id)) == 0 && time.Now().Before(deadline) {
		time.Sleep(2 * time.Millisecond)
	}
	if len(h.reviews(id)) != 1 {
		t.Fatal("the loop did not review the new proposal")
	}
}

func TestNewRejectsBadSettings(t *testing.T) {
	for name, mut := range map[string]func(*config.Review){
		"glob":  func(r *config.Review) { r.Protected = []string{"a["} },
		"regex": func(r *config.Review) { r.AllowRE = []string{"("} },
	} {
		rv := config.Review{Enabled: on(true), MaxFiles: 1, MaxLines: 1}
		mut(&rv)
		if _, err := New(Options{Repo: config.Repo{Name: repo, Review: rv}}); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestRenderMentionsTruncationAndNeverClaimsApproval(t *testing.T) {
	out := Render("abcdef0123456789", "service:reviewer", []Finding{{Kind: KindOversized, Severity: SevWarn, Title: "Large change", Details: []string{"x"}}}, Summary{Files: 2, Added: 3, Removed: 4, Truncated: true}, "dg")
	for _, want := range []string{"truncated", "+3 −4", "service:reviewer", "neither an approval nor a request for changes", "digest=dg"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q:\n%s", want, out)
		}
	}
}
