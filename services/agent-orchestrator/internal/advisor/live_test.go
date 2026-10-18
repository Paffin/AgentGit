package advisor

import (
	"context"
	"io"
	"log/slog"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// Opt-in (AG_LLM_LIVE=1, see testutil.LiveLLM): a real ag-server holds a real conflicting proposal, a real model is
// asked to propose a resolution, and what lands on the server is a comment, never a change.
func TestLiveModelProposesAResolutionForARealConflict(t *testing.T) {
	rec := testutil.LiveLLM(t)
	base := map[string]string{"calc/total.go": "package calc\n\n// Total adds up the amounts.\nfunc Total(amounts []int) int {\n\tsum := 0\n\tfor _, a := range amounts {\n\t\tsum += a\n\t}\n\treturn sum\n}\n"}
	ours := map[string]string{"calc/total.go": "package calc\n\n// Total adds up the amounts; negative amounts (refunds) are ignored.\nfunc Total(amounts []int) int {\n\tsum := 0\n\tfor _, a := range amounts {\n\t\tif a < 0 {\n\t\t\tcontinue\n\t\t}\n\t\tsum += a\n\t}\n\treturn sum\n}\n"}
	theirs := map[string]string{"calc/total.go": "package calc\n\n// Total adds up the amounts; amounts above the cap are clamped to it.\nfunc Total(amounts []int) int {\n\tconst cap = 1000\n\tsum := 0\n\tfor _, a := range amounts {\n\t\tif a > cap {\n\t\t\ta = cap\n\t\t}\n\t\tsum += a\n\t}\n\treturn sum\n}\n"}
	h := newScenario(t, nil, base, ours, theirs, nil, nil)
	h.comp = New(Options{Repo: h.comp.o.Repo, API: h.client, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
		Advisor: ModelAdvisor{LLM: rec, Rules: Rules{MaxFileBytes: h.comp.o.Repo.Advisor.MaxFile}}})

	if err := h.comp.Process(context.Background(), h.prepared("calc/total.go")); err != nil {
		t.Fatal(err)
	}
	calls := rec.Calls()
	if len(calls) != 1 {
		t.Fatalf("%d model calls, want exactly one for one conflicted file", len(calls))
	}
	r, ok := h.comp.Suggestions(h.prop.ID)
	if !ok || len(r.Suggestions) != 1 {
		t.Fatalf("record = %+v", r)
	}
	s := r.Suggestions[0]
	t.Logf("suggestion: advisor=%s class=%s complete=%v confidence=%s notes=%q", s.Advisor, s.Class, s.Complete, s.Confidence, s.Notes)
	if s.Advisor == "model/"+rec.Model()+"+rules/1" || strings.HasPrefix(s.Advisor, "model/") {
		if hasConflictMarkers(string(s.Resolved)) || s.Confidence == High {
			t.Errorf("an accepted model suggestion must be marker-free and rated at most medium: %+v", s)
		}
	} else if !strings.Contains(strings.Join(s.Notes, "\n"), "model") {
		t.Errorf("the rules' answer stands but does not say what happened with the model: %+v", s)
	}
	// it is a comment, and nothing was applied
	rs := h.reviews()
	if len(rs) != 1 || rs[0].Decision != agwp.DecisionComment || rs[0].Reviewer != "service:advisor" {
		t.Fatalf("reviews = %+v", rs)
	}
	d, err := h.w.Alice.GetProposal(context.Background(), repo, h.prop.ID)
	if err != nil {
		t.Fatal(err)
	}
	if d.Proposal.Head != h.theirs || d.Proposal.State != agwp.ProposalOpen || h.w.Head("main") != h.ours {
		t.Fatalf("the proposal or the stream moved: proposal %+v, main %s (was %s)", d.Proposal, h.w.Head("main"), h.ours)
	}
	t.Logf("comment posted on the proposal (%d bytes):\n%s", len(rs[0].Body), rs[0].Body)
}
