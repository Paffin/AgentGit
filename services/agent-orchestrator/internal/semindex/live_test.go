package semindex

import (
	"context"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// Opt-in (AG_LLM_LIVE=1, see testutil.LiveLLM): two real open proposals on a real ag-server that the static analysis
// finds overlapping, and a real model's opinion on whether the overlap matters. The static severity is untouched.
func TestLiveModelAssessesARealOverlap(t *testing.T) {
	rec := testutil.LiveLLM(t)
	h := newHarness(t, nil)
	h.judge = LLMJudge{LLM: rec}
	h.comp = h.newComponent()
	seed := h.setTree(overlapBase)
	// A makes Fetch answer "" for an empty id; B makes Fetch's callers depend on a non-empty answer
	a := h.propose(seed, "Fetch: tolerate an empty id", map[string]string{"pkg/api/api.go": "package api\n\n// Fetch returns the record for id, or \"\" when id is empty.\nfunc Fetch(id string) string {\n\tif id == \"\" {\n\t\treturn \"\"\n\t}\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	b := h.propose(seed, "Store: index by Fetch", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id\n}\n\n// Store files v under the key Fetch(v); the key must never be empty.\nfunc Store(v string) {\n\tkey := Fetch(v)\n\tindex[key[:1]] = v\n}\n\nvar index = map[string]string{}\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	var def *Overlap
	ov := h.comp.Overlaps()
	for i := range ov {
		t.Logf("overlap %s/%s kind=%s static=%s: %s", ov[i].A, ov[i].B, ov[i].Kind, ov[i].Severity, ov[i].Why)
		if ov[i].Kind == KindDefinition {
			def = &ov[i]
		}
	}
	if def == nil || !((def.A == a && def.B == b) || (def.A == b && def.B == a)) {
		t.Fatalf("the static analysis did not find the overlap: %+v", ov)
	}
	st := h.comp.Status()
	if st.JudgeError != "" || def.Assessment == nil {
		t.Fatalf("the live judgement failed: %q (assessment %+v)", st.JudgeError, def.Assessment)
	}
	as := def.Assessment
	t.Logf("assessment: model=%s verdict=%s severity=%s source=%q why=%s", as.Model, as.Verdict, as.Severity, as.Source, as.Why)
	if as.Source != "language model" || as.Model == "" || as.Why == "" {
		t.Errorf("assessment = %+v", as)
	}
	if def.Severity == "" {
		t.Error("the static severity went missing")
	}
}
