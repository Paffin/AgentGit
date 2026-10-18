package testutil

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
)

// LLMRequest is a chat-completions request as the LLMServer received it.
type LLMRequest struct {
	N         int // 1-based
	Model     string
	Messages  []llm.Message
	MaxTokens int
	Header    http.Header
	Raw       string
}

// System is the content of the system message ("" when there is none).
func (r LLMRequest) System() string {
	for _, m := range r.Messages {
		if m.Role == llm.System {
			return m.Content
		}
	}
	return ""
}

// User is the content of the last user message.
func (r LLMRequest) User() string {
	for i := len(r.Messages) - 1; i >= 0; i-- {
		if r.Messages[i].Role == llm.User {
			return r.Messages[i].Content
		}
	}
	return ""
}

// LLMReply is what the LLMServer answers.
type LLMReply struct {
	Text             string
	Status           int // 0: 200
	PromptTokens     int
	CompletionTokens int
	Finish           string // default "stop"
}

// LLMServer is a TEST DOUBLE OF A THIRD-PARTY API: an OpenAI-compatible chat-completions endpoint. A language model
// cannot run inside a test, so the tests that exercise the model-backed components answer their requests from a
// script and check what was SENT (the prompts, their fencing, the bounds) and what the components make of the
// answers (validation, fallbacks). What a real provider does with the prompts is checked live and opt-in
// (AG_LLM_LIVE=1, see internal/llm/live_test.go and the live tests of each component).
type LLMServer struct {
	URL string // the API root to configure as llm.base_url

	mu    sync.Mutex
	reqs  []LLMRequest
	reply func(LLMRequest) LLMReply
}

// NewLLMServer starts the double. reply is called once per request.
func NewLLMServer(t testing.TB, reply func(LLMRequest) LLMReply) *LLMServer {
	t.Helper()
	s := &LLMServer{reply: reply}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var body struct {
			Model     string        `json:"model"`
			Messages  []llm.Message `json:"messages"`
			MaxTokens int           `json:"max_tokens"`
		}
		_ = json.Unmarshal(b, &body)
		s.mu.Lock()
		req := LLMRequest{N: len(s.reqs) + 1, Model: body.Model, Messages: body.Messages, MaxTokens: body.MaxTokens, Header: r.Header.Clone(), Raw: string(b)}
		s.reqs = append(s.reqs, req)
		s.mu.Unlock()
		rep := s.reply(req)
		if rep.Status != 0 && rep.Status/100 != 2 {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(rep.Status)
			_ = json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"message": rep.Text, "code": rep.Status}})
			return
		}
		finish := rep.Finish
		if finish == "" {
			finish = "stop"
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id": "gen-test", "model": body.Model,
			"choices": []any{map[string]any{"finish_reason": finish, "message": map[string]any{"role": "assistant", "content": rep.Text}}},
			"usage":   map[string]any{"prompt_tokens": max(rep.PromptTokens, 1), "completion_tokens": max(rep.CompletionTokens, 1)},
		})
	}))
	t.Cleanup(srv.Close)
	s.URL = srv.URL + "/v1"
	return s
}

// Requests is a copy of the requests received so far.
func (s *LLMServer) Requests() []LLMRequest {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]LLMRequest(nil), s.reqs...)
}

// Count is how many requests were received.
func (s *LLMServer) Count() int { return len(s.Requests()) }
