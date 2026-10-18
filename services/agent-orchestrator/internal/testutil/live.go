package testutil

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// The live tests talk to a REAL language-model endpoint. They are opt-in (a model costs money and needs a network):
//
//	AG_LLM_LIVE=1                       switches them on
//	AG_LLM_LIVE_MODEL=<model id>        required: the model to ask (a cheap one is plenty)
//	AG_LLM_LIVE_BASE_URL=<api root>     default https://openrouter.ai/api/v1
//	AG_LLM_LIVE_KEY_ENV=<VAR NAME>      the NAME of the variable holding the key; unset: no Authorization header is sent
//	                                    (a sandbox or gateway proxy that injects it needs none)
//
// Nothing here carries a key; the key, when there is one, is read from the named variable only.
const (
	liveSwitch  = "AG_LLM_LIVE"
	liveModel   = "AG_LLM_LIVE_MODEL"
	liveBaseURL = "AG_LLM_LIVE_BASE_URL"
	liveKeyEnv  = "AG_LLM_LIVE_KEY_ENV"
)

// LiveCall is one recorded exchange: the SHAPE of the prompt (sizes and the names of the fenced pieces, not the
// repository text) and the model's answer.
type LiveCall struct {
	Purpose      string
	SystemBytes  int
	UserBytes    int
	Fenced       []string
	Reply        string
	ModelID      string // as the provider reports it
	Usage        llm.Usage
	FinishReason string
	Attempts     int
	Took         time.Duration
}

// LiveRecorder wraps the real client and keeps the transcript of the calls for the evidence in the docs.
type LiveRecorder struct {
	*llm.Client
	t     testing.TB
	mu    sync.Mutex
	calls []LiveCall
}

var fenceRE = regexp.MustCompile(`<<<UNTRUSTED id=\S+ name="([^"]*)" bytes=(\d+)`)

// LiveSettings are the live endpoint's settings as an orchestrator configuration names them: the API root, the model
// and the NAME of the key variable ("" when none is needed). It skips the test when live tests are off.
func LiveSettings(t testing.TB) (baseURL, model, keyEnv string) {
	t.Helper()
	if os.Getenv(liveSwitch) != "1" {
		t.Skip("live model test: set " + liveSwitch + "=1 (and " + liveModel + ") to run it against a real endpoint")
	}
	model = os.Getenv(liveModel)
	if model == "" {
		t.Fatalf("%s=1 needs %s: the model is your choice, nothing is assumed", liveSwitch, liveModel)
	}
	baseURL = os.Getenv(liveBaseURL)
	if baseURL == "" {
		baseURL = "https://openrouter.ai/api/v1"
	}
	return baseURL, model, os.Getenv(liveKeyEnv)
}

// LiveLLM returns a recording client for the configured live endpoint, or skips the test when live tests are off.
func LiveLLM(t testing.TB) *LiveRecorder {
	t.Helper()
	if os.Getenv(liveSwitch) != "1" {
		t.Skip("live model test: set " + liveSwitch + "=1 (and " + liveModel + ") to run it against a real endpoint")
	}
	model := os.Getenv(liveModel)
	if model == "" {
		t.Fatalf("%s=1 needs %s: the model is your choice, nothing is assumed", liveSwitch, liveModel)
	}
	base := os.Getenv(liveBaseURL)
	if base == "" {
		base = "https://openrouter.ai/api/v1"
	}
	var key secret.Secret
	if name := os.Getenv(liveKeyEnv); name != "" {
		key = secret.New(os.Getenv(name))
	}
	c, err := llm.New(llm.Config{BaseURL: base, Model: model, APIKey: key, Timeout: 120 * time.Second, Retries: 3, MaxOutputTokens: 1500, MaxInputBytes: 120_000,
		Concurrency: 2, Budget: llm.Budget{RequestsPerHour: 100, TokensPerDay: 400_000}, Log: slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))})
	if err != nil {
		t.Fatal(err)
	}
	r := &LiveRecorder{Client: c, t: t}
	t.Cleanup(r.Report)
	return r
}

// Complete implements llm.Completer.
func (r *LiveRecorder) Complete(ctx context.Context, req llm.Request) (*llm.Response, error) {
	call := LiveCall{Purpose: req.Purpose}
	for _, m := range req.Messages {
		if m.Role == llm.System {
			call.SystemBytes += len(m.Content)
		} else {
			call.UserBytes += len(m.Content)
			for _, f := range fenceRE.FindAllStringSubmatch(m.Content, -1) {
				call.Fenced = append(call.Fenced, fmt.Sprintf("%s (%s bytes)", f[1], f[2]))
			}
		}
	}
	resp, err := r.Client.Complete(ctx, req)
	if resp != nil {
		call.Reply, call.ModelID, call.Usage, call.FinishReason, call.Attempts, call.Took = resp.Text, resp.Model, resp.Usage, resp.FinishReason, resp.Attempts, resp.Duration
	}
	r.mu.Lock()
	r.calls = append(r.calls, call)
	r.mu.Unlock()
	return resp, err
}

// Calls is a copy of the transcript.
func (r *LiveRecorder) Calls() []LiveCall {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]LiveCall(nil), r.calls...)
}

// Report prints the transcript (go test -v shows it).
func (r *LiveRecorder) Report() {
	var b strings.Builder
	tot := r.Totals()
	fmt.Fprintf(&b, "LIVE TRANSCRIPT model=%s requests=%d prompt_tokens=%d completion_tokens=%d\n", r.Model(), tot.Requests, tot.PromptTokens, tot.CompletionTokens)
	for i, c := range r.Calls() {
		fmt.Fprintf(&b, "call %d purpose=%s provider_model=%s system=%dB user=%dB fenced=%v tokens=%d+%d finish=%s attempts=%d took=%s\nreply: %s\n",
			i+1, c.Purpose, c.ModelID, c.SystemBytes, c.UserBytes, c.Fenced, c.Usage.PromptTokens, c.Usage.CompletionTokens, c.FinishReason, c.Attempts, c.Took.Round(time.Millisecond), strings.TrimSpace(c.Reply))
	}
	r.t.Log(b.String())
}
