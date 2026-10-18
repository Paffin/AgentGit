package app

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// Model mode, end to end through the application wiring: the configuration names the endpoint, the model and the
// environment variable of the key; the reviewer asks the (test double of the) model about a real proposal on a real
// ag-server, and the comment that lands there says which model wrote which part. The key reaches the endpoint only
// as the Authorization header and appears nowhere in /status or in the comment.
func TestModelModeWiresTheReviewerToTheConfiguredEndpoint(t *testing.T) {
	const key = "sk-test-key-0123456789abcdef"
	srv := testutil.NewLLMServer(t, func(testutil.LLMRequest) testutil.LLMReply {
		return testutil.LLMReply{Text: `{"summary":"adds a handler","findings":[{"severity":"warning","title":"Unchecked error","details":"the error from Open is dropped","path":"pkg/h.go","line":4}]}`, PromptTokens: 321, CompletionTokens: 45}
	})
	r := newRig(t, func(r *testRig) {
		r.cfg.LLM.BaseURL = srv.URL
		r.cfg.LLM.Model = "vendor/some-model"
		r.cfg.LLM.APIKeyEnv = "AG_LLM_KEY"
		r.envv["AG_LLM_KEY"] = key
		r.cfg.Repos[0].Review.Mode = config.ModeModel
	})
	p, _ := r.w.Propose("ws-h", "adds a handler", testutil.Put("pkg/h.go", "package pkg\n\nfunc H() {\n\tos.Open(\"x\")\n}\n"))
	r.start(t)
	t.Cleanup(func() {
		if t.Failed() {
			t.Logf("model requests: %d, review status: %+v", srv.Count(), *r.app.Status().Repos[0].Review)
		}
	})
	var body string
	eventually(t, "the review comment", 30*time.Second, func() bool {
		d, err := r.w.Alice.GetProposal(context.Background(), repoName, p.ID)
		if err != nil || len(d.Reviews) == 0 {
			return false
		}
		body = d.Reviews[0].Body
		return true
	})
	if !strings.Contains(body, "vendor/some-model") || !strings.Contains(body, "Unchecked error") {
		t.Fatalf("the comment does not carry the model's findings under its name:\n%s", body)
	}
	reqs := srv.Requests()
	if len(reqs) == 0 || reqs[0].Header.Get("Authorization") != "Bearer "+key || reqs[0].Model != "vendor/some-model" {
		t.Fatalf("the request was not made as configured: %+v", reqs)
	}
	_, st := r.get(t, "/status", "")
	if strings.Contains(st, key) || !strings.Contains(st, `"llm"`) || !strings.Contains(st, "vendor/some-model") || !strings.Contains(st, `"prompt_tokens": 321`) {
		t.Fatalf("status does not report the model use (or leaks the key):\n%s", st)
	}
	if strings.Contains(body, key) {
		t.Fatal("the key leaked into a comment")
	}
}
