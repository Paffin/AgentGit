package review

import (
	"context"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// Opt-in (AG_LLM_LIVE=1, see testutil.LiveLLM): a real ag-server holds a real proposal with a real defect, a real
// model reads its diff, and what lands on the server is a comment that says who wrote what.
func TestLiveModelReadsARealProposal(t *testing.T) {
	rec := testutil.LiveLLM(t)
	h := newHarness(t, false)
	h.c.o.Model = LLMReviewer{LLM: rec}
	const users = "package store\n\nimport (\n\t\"database/sql\"\n\t\"fmt\"\n)\n\n// FindUser returns the id of the user with the given name.\nfunc FindUser(db *sql.DB, name string) (int, error) {\n\tvar id int\n\trow := db.QueryRow(\"SELECT id FROM users WHERE name = '\" + name + \"'\")\n\trow.Scan(&id)\n\tfmt.Println(\"found\", id)\n\treturn id, nil\n}\n"
	id, head := h.propose("add FindUser", map[string]string{"store/users.go": users})
	h.reconcile()
	rs := h.reviews(id)
	if len(rs) != 1 || rs[0].Decision != agwp.DecisionComment || rs[0].Head != head {
		t.Fatalf("reviews = %+v", rs)
	}
	body := rs[0].Body
	r := h.c.Status().Recent[0]
	t.Logf("model=%s summary=%q findings=%d error=%q", r.Model, r.ModelSummary, len(r.ModelFindings), r.ModelError)
	for _, f := range r.ModelFindings {
		t.Logf("  finding: %s %s: %v", f.Severity, f.Title, f.Details)
	}
	if r.ModelError != "" {
		t.Fatalf("the live review fell back to the heuristic reviewer: %s", r.ModelError)
	}
	if !strings.Contains(body, "Language-model review (`model/"+rec.Model()+"`)") || !strings.Contains(body, "it cannot approve or block anything") {
		t.Errorf("the comment does not say who wrote the model's section:\n%s", body)
	}
	if len(r.ModelFindings) == 0 {
		t.Errorf("the model found nothing in a query built by concatenating user input and an ignored Scan error")
	}
	// the model's words are text in a comment: the proposal is untouched
	d, err := h.w.Alice.GetProposal(context.Background(), repo, id)
	if err != nil {
		t.Fatal(err)
	}
	if d.Proposal.State != agwp.ProposalOpen || d.Proposal.Head != head {
		t.Fatalf("proposal = %+v", d.Proposal)
	}
	t.Logf("comment posted on the proposal (%d bytes):\n%s", len(body), body)
}
