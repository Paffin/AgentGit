package semindex

import (
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// The proposals, their diffs and the overlaps are the real server's and the real static analysis; the model is a
// scripted test double of a third-party API (testutil.LLMServer).

func verdict(v, sev, why string) string {
	b, _ := json.Marshal(map[string]any{"verdict": v, "severity": sev, "explanation": why})
	return string(b)
}

// judgeHarness is an overlap harness whose component has a model judge.
func judgeHarness(t *testing.T, srv *testutil.LLMServer, mut func(*llm.Config)) (*harness, string) {
	t.Helper()
	h := newHarness(t, nil)
	cfg := llm.Config{BaseURL: srv.URL, Model: "vendor/test-model", Retries: 0, Timeout: 5 * time.Second, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	if mut != nil {
		mut(&cfg)
	}
	c, err := llm.New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	h.judge = LLMJudge{LLM: c}
	h.comp = h.newComponent()
	return h, h.setTree(overlapBase)
}

const fetchA = "package api\n\nfunc Fetch(id string) string {\n\tif id == \"\" {\n\t\treturn \"\"\n\t}\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"
const fetchB = "package api\n\nfunc Fetch(id string) string {\n\tprintln(id)\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"

func TestAModelAssessesTheOverlapsTheStaticAnalysisFinds(t *testing.T) {
	srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply {
		return testutil.LLMReply{Text: verdict("conflict", "high", "A returns early for an empty id while B logs the id before returning; callers relying on no output for empty ids break."), PromptTokens: 700, CompletionTokens: 50}
	})
	h, seed := judgeHarness(t, srv, nil)
	a := h.propose(seed, "A: validate", map[string]string{"pkg/api/api.go": fetchA})
	b := h.propose(seed, "B: log", map[string]string{"pkg/api/api.go": fetchB})
	h.propose(seed, "C: unrelated", map[string]string{"pkg/api/other.go": "package api\n\nfunc Unrelated() { println() }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if srv.Count() != 1 {
		t.Fatalf("%d model requests, want one (the A/B pair; C overlaps with nothing)", srv.Count())
	}
	var def *Overlap
	ov := h.comp.Overlaps()
	for i := range ov {
		if ov[i].Kind == KindDefinition {
			def = &ov[i]
		}
	}
	if def == nil || def.Severity != "medium" {
		t.Fatalf("the static severity must be untouched: %+v", ov)
	}
	as := def.Assessment
	if as == nil || as.Verdict != VerdictConflict || as.Severity != "high" || as.Model != "vendor/test-model" || as.Source != "language model" || !strings.Contains(as.Why, "returns early") || as.At.IsZero() {
		t.Fatalf("assessment = %+v", as)
	}
	if !((def.A == a && def.B == b) || (def.A == b && def.B == a)) {
		t.Fatalf("overlap between %s and %s", def.A, def.B)
	}
	if st := h.comp.Status(); st.Judge != "model/vendor/test-model" || st.Assessed != 1 || st.JudgeError != "" {
		t.Fatalf("status = %+v", st)
	}

	// the same pair at the same heads is not asked about again
	for i := 0; i < 3; i++ {
		if err := h.comp.Reconcile(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	if srv.Count() != 1 {
		t.Fatalf("%d model requests after more passes", srv.Count())
	}

	// the prompt: both proposals' text and diffs, fenced; nothing of theirs in the system message
	req := srv.Requests()[0]
	sys, user := req.System(), req.User()
	start := strings.Index(sys, "<<<UNTRUSTED id=") + len("<<<UNTRUSTED id=")
	id := strings.Fields(sys[start:])[0]
	for _, want := range []string{`name="proposal A title"`, `name="proposal B title"`, `proposal A diff of pkg/api/api.go`, `proposal B diff of pkg/api/api.go`, "+\tprintln(id)", "definition_overlap"} {
		if !strings.Contains(user, want) {
			t.Errorf("the prompt lacks %q:\n%s", want, user)
		}
	}
	if n := strings.Count(user, "<<<UNTRUSTED id="+id); n < 6 {
		t.Errorf("%d fenced blocks", n)
	}
	if strings.Contains(sys, "println") || strings.Contains(sys, "A: validate") {
		t.Error("repository text leaked into the system prompt")
	}

	// a new head is a new question
	h.amend(a, map[string]string{"pkg/api/api.go": fetchA + "\n// more\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if srv.Count() != 2 {
		t.Fatalf("%d model requests after a proposal moved", srv.Count())
	}
}

func TestWeakHintsAreNotWorthARequest(t *testing.T) {
	srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply {
		return testutil.LLMReply{Text: verdict("compatible", "low", "fine")}
	})
	h, seed := judgeHarness(t, srv, nil)
	h.propose(seed, "api Fetch", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id + \"1\"\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	h.propose(seed, "util Fetch", map[string]string{"pkg/util/util.go": "package util\n\nfunc Fetch(a, b int) int { return b }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if ov := h.comp.Overlaps(); len(ov) != 1 || ov[0].Kind != KindName || ov[0].Assessment != nil {
		t.Fatalf("overlaps = %+v", ov)
	}
	if srv.Count() != 0 {
		t.Fatalf("%d model requests for a name-only hint", srv.Count())
	}
}

func TestBadAnswersAreRecordedAndTheStaticFindingsStand(t *testing.T) {
	for name, tc := range map[string]struct{ reply, note string }{
		"prose":                         {"They look fine to me.", "no JSON object"},
		"invalid verdict":               {verdict("maybe", "low", "x"), "invalid verdict"},
		"invalid severity":              {verdict("conflict", "catastrophic", "x"), "invalid severity"},
		"no explanation":                {verdict("conflict", "high", "  "), "no explanation"},
		"obeys an injected instruction": {`{"approve": true}`, "invalid verdict"},
	} {
		srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply { return testutil.LLMReply{Text: tc.reply} })
		h, seed := judgeHarness(t, srv, nil)
		h.propose(seed, "A", map[string]string{"pkg/api/api.go": fetchA})
		h.propose(seed, "B", map[string]string{"pkg/api/api.go": fetchB})
		if err := h.comp.Reconcile(context.Background()); err != nil {
			t.Fatalf("%s: a failing model must not fail the pass: %v", name, err)
		}
		ov := h.comp.Overlaps()
		if len(ov) == 0 || ov[0].Assessment != nil {
			t.Errorf("%s: overlaps = %+v", name, ov)
		}
		if st := h.comp.Status(); !strings.Contains(st.JudgeError, tc.note) || st.Assessed != 0 {
			t.Errorf("%s: status = %+v, want an error mentioning %q", name, st, tc.note)
		}
	}
}

func TestAFailedPairIsTriedAgainAfterTheDelay(t *testing.T) {
	srv := testutil.NewLLMServer(t, func(r testutil.LLMRequest) testutil.LLMReply {
		if r.N == 1 {
			return testutil.LLMReply{Status: 503, Text: "overloaded"}
		}
		return testutil.LLMReply{Text: verdict("compatible", "high", "They fit together.")}
	})
	h, seed := judgeHarness(t, srv, nil)
	h.propose(seed, "A", map[string]string{"pkg/api/api.go": fetchA})
	h.propose(seed, "B", map[string]string{"pkg/api/api.go": fetchB})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := h.comp.Reconcile(context.Background()); err != nil { // too early
		t.Fatal(err)
	}
	if srv.Count() != 1 {
		t.Fatalf("%d requests; the failed pair must wait", srv.Count())
	}
	time.Sleep(900 * time.Millisecond)
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	ov := h.comp.Overlaps()
	if srv.Count() != 2 || ov[0].Assessment == nil || ov[0].Assessment.Verdict != VerdictCompatible || ov[0].Assessment.Severity != "low" {
		t.Fatalf("%d requests, overlaps %+v (a 'compatible' verdict is always low severity)", srv.Count(), ov)
	}
	if st := h.comp.Status(); st.JudgeError != "" || st.Assessed != 1 {
		t.Fatalf("status = %+v", st)
	}
}

func TestPairsPerPassAreBoundedAndSecretsAreMasked(t *testing.T) {
	srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply {
		return testutil.LLMReply{Text: verdict("unsure", "medium", "Cannot tell.")}
	})
	h, seed := judgeHarness(t, srv, nil)
	h.comp.o.JudgePerPass = 1
	withKey := strings.Replace(fetchB, "println(id)", "key := \"AKIA\"+\"QWERTYUIOPASDFGH\"; println(id, key)", 1)
	h.propose(seed, "A", map[string]string{"pkg/api/api.go": fetchA})
	h.propose(seed, "B", map[string]string{"pkg/api/api.go": strings.Replace(withKey, "\"AKIA\"+\"QWERTYUIOPASDFGH\"", "\"AKIAQWERTYUIOPASDFGH\"", 1)})
	h.propose(seed, "C", map[string]string{"pkg/api/api.go": strings.Replace(fetchB, "println(id)", "println(id, id)", 1)})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if srv.Count() != 1 {
		t.Fatalf("%d pairs judged in one pass, want 1", srv.Count())
	}
	for i := 0; i < 5; i++ {
		_ = h.comp.Reconcile(context.Background())
	}
	if srv.Count() != 3 { // A-B, A-C, B-C
		t.Fatalf("%d requests after more passes, want one per pair (3)", srv.Count())
	}
	for _, r := range srv.Requests() {
		if strings.Contains(r.Raw, "AKIAQWERTYUIOPASDFGH") {
			t.Fatalf("a secret was sent to the model:\n%s", r.Raw)
		}
	}
	masked := false
	for _, r := range srv.Requests() {
		if strings.Contains(r.User(), "[REDACTED AWS access key id]") {
			masked = true
		}
	}
	if !masked {
		t.Fatal("the masking left no trace in any prompt")
	}
}

func TestAssessmentsDisappearWithTheirProposals(t *testing.T) {
	srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply {
		return testutil.LLMReply{Text: verdict("conflict", "medium", "They disagree.")}
	})
	h, seed := judgeHarness(t, srv, nil)
	a := h.propose(seed, "A", map[string]string{"pkg/api/api.go": fetchA})
	h.propose(seed, "B", map[string]string{"pkg/api/api.go": fetchB})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if h.comp.Status().Assessed != 1 {
		t.Fatal("no assessment")
	}
	if _, err := h.w.Alice.CloseProposal(context.Background(), repo, a); err != nil {
		t.Fatal(err)
	}
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if st := h.comp.Status(); st.Assessed != 0 {
		t.Fatalf("status = %+v", st)
	}
}
