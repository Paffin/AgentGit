package app

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// Opt-in (AG_LLM_LIVE=1, see testutil.LiveSettings): the whole application, configured the way an operator would
// (endpoint, model and the NAME of the key variable), a real ag-server, a real proposal with a real defect and a real
// model. The comment on the proposal is the evidence.
func TestLiveApplicationReviewsAProposalWithARealModel(t *testing.T) {
	base, model, keyEnv := testutil.LiveSettings(t)
	r := newRig(t, func(r *testRig) {
		r.cfg.LLM.BaseURL, r.cfg.LLM.Model, r.cfg.LLM.APIKeyEnv = base, model, keyEnv
		if keyEnv != "" {
			r.envv[keyEnv] = os.Getenv(keyEnv)
		}
		r.cfg.Repos[0].Review.Mode = config.ModeModel
		r.cfg.Repos[0].Index.Mode = config.ModeModel
	})
	if err := r.cfg.Validate(r.getenv); err != nil {
		t.Fatalf("the live configuration is not valid: %v", err)
	}
	const users = "package store\n\nimport (\n\t\"database/sql\"\n)\n\n// Delete removes the user with the given name.\nfunc Delete(db *sql.DB, name string) {\n\tdb.Exec(\"DELETE FROM users WHERE name = '\" + name + \"'\")\n}\n"
	p, _ := r.w.Propose("ws-del", "add Delete", testutil.Put("store/users.go", users))
	r.start(t)
	var body string
	eventually(t, "the review comment", 90*time.Second, func() bool {
		d, err := r.w.Alice.GetProposal(context.Background(), repoName, p.ID)
		if err != nil || len(d.Reviews) == 0 {
			return false
		}
		body = d.Reviews[0].Body
		return true
	})
	t.Logf("comment:\n%s", body)
	if !strings.Contains(body, "Language-model review (`model/"+model+"`)") {
		t.Errorf("the comment has no model section:\n%s", body)
	}
	_, st := r.get(t, "/status", "")
	if i := strings.Index(st, `"llm"`); i < 0 {
		t.Error("/status has no llm section")
	} else {
		t.Logf("status llm section:\n%s", st[i:min(len(st), i+260)])
	}
	if keyEnv != "" && strings.Contains(st, r.getenv(keyEnv)) {
		t.Fatal("the key is in /status")
	}
}
