package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// The server in these tests is a TEST DOUBLE OF A THIRD-PARTY API (an OpenAI-compatible chat-completions
// endpoint): no such service can run inside a test. It records what the client sent and answers what the test
// scripts. Everything about the real behaviour of a provider is checked live (internal/llm/live_test.go, opt-in).

const testKey = "sk-test-0123456789abcdef-SECRET"

// stepClock is a clock whose sleeps return at once and are recorded; its time moves only when told to.
type stepClock struct {
	mu     sync.Mutex
	now    time.Time
	sleeps []time.Duration
}

func newStepClock() *stepClock { return &stepClock{now: time.Unix(1_800_000_000, 0)} }

func (c *stepClock) Now() time.Time { c.mu.Lock(); defer c.mu.Unlock(); return c.now }
func (c *stepClock) advance(d time.Duration) {
	c.mu.Lock()
	c.now = c.now.Add(d)
	c.mu.Unlock()
}
func (c *stepClock) NewTimer(d time.Duration) clock.Timer {
	c.mu.Lock()
	c.sleeps = append(c.sleeps, d)
	c.mu.Unlock()
	ch := make(chan time.Time, 1)
	ch <- c.Now()
	return fired{ch}
}
func (c *stepClock) NewTicker(time.Duration) clock.Ticker { panic("not used") }
func (c *stepClock) recorded() []time.Duration {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]time.Duration(nil), c.sleeps...)
}

type fired struct{ ch chan time.Time }

func (f fired) C() <-chan time.Time { return f.ch }
func (f fired) Stop() bool          { return false }

type seen struct {
	method, path string
	header       http.Header
	body         chatRequest
	raw          string
}

type provider struct {
	*httptest.Server
	mu    sync.Mutex
	seen  []seen
	reply func(n int, w http.ResponseWriter, r *http.Request) // n counts from 1
}

func newProvider(t *testing.T, reply func(n int, w http.ResponseWriter, r *http.Request)) *provider {
	t.Helper()
	p := &provider{reply: reply}
	p.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var cr chatRequest
		_ = json.Unmarshal(b, &cr)
		p.mu.Lock()
		p.seen = append(p.seen, seen{r.Method, r.URL.Path, r.Header.Clone(), cr, string(b)})
		n := len(p.seen)
		p.mu.Unlock()
		p.reply(n, w, r)
	}))
	t.Cleanup(p.Close)
	return p
}

func (p *provider) requests() []seen {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]seen(nil), p.seen...)
}

func ok(text string, prompt, completion int) func(int, http.ResponseWriter, *http.Request) {
	return func(_ int, w http.ResponseWriter, _ *http.Request) { writeOK(w, text, "stop", prompt, completion) }
}

func writeOK(w http.ResponseWriter, text, finish string, prompt, completion int) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"id": "gen-1", "model": "vendor/model-x",
		"choices": []any{map[string]any{"finish_reason": finish, "message": map[string]any{"role": "assistant", "content": text}}},
		"usage":   map[string]any{"prompt_tokens": prompt, "completion_tokens": completion},
	})
}

func newClient(t *testing.T, p *provider, mut func(*Config)) (*Client, *stepClock) {
	t.Helper()
	ck := newStepClock()
	cfg := Config{BaseURL: p.URL + "/v1", Model: "vendor/model-x", APIKey: secret.New(testKey), Clock: ck, Retries: 3,
		Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	if mut != nil {
		mut(&cfg)
	}
	c, err := New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	return c, ck
}

var hello = Request{Purpose: "test", Messages: []Message{{Role: System, Content: "sys"}, {Role: User, Content: "hi"}}}

func TestRequestShapeAndResponse(t *testing.T) {
	p := newProvider(t, ok(`{"a":1}`, 11, 7))
	c, _ := newClient(t, p, func(c *Config) {
		c.MaxOutputTokens, c.Temperature, c.JSONMode = 333, 0.25, true
		c.Headers = map[string]string{"X-Title": "agentgit-test"}
	})
	r, err := c.Complete(context.Background(), hello)
	if err != nil {
		t.Fatal(err)
	}
	if r.Text != `{"a":1}` || r.Model != "vendor/model-x" || r.Usage.PromptTokens != 11 || r.Usage.CompletionTokens != 7 || r.Usage.Estimated || r.RequestID != "gen-1" || r.Attempts != 1 {
		t.Fatalf("response = %+v", r)
	}
	reqs := p.requests()
	if len(reqs) != 1 {
		t.Fatalf("%d requests", len(reqs))
	}
	q := reqs[0]
	if q.method != "POST" || q.path != "/v1/chat/completions" {
		t.Fatalf("%s %s", q.method, q.path)
	}
	if q.header.Get("Authorization") != "Bearer "+testKey || q.header.Get("Content-Type") != "application/json" || q.header.Get("X-Title") != "agentgit-test" {
		t.Fatalf("headers = %v", q.header)
	}
	b := q.body
	if b.Model != "vendor/model-x" || b.MaxTokens != 333 || b.Temperature != 0.25 || len(b.Messages) != 2 || b.Messages[1].Content != "hi" || b.ResponseFormat["type"] != "json_object" {
		t.Fatalf("body = %+v", b)
	}
	if tot := c.Totals(); tot.Requests != 1 || tot.PromptTokens != 11 || tot.CompletionTokens != 7 || tot.TokensLastDay != 18 || tot.RequestsLastHour != 1 {
		t.Fatalf("totals = %+v", tot)
	}
}

func TestNoKeyMeansNoAuthorizationHeader(t *testing.T) {
	// a gateway that injects the credential itself (a corporate or sandbox egress proxy, a local server)
	p := newProvider(t, ok("hello", 1, 1))
	c, _ := newClient(t, p, func(c *Config) { c.APIKey = secret.Secret{} })
	if _, err := c.Complete(context.Background(), hello); err != nil {
		t.Fatal(err)
	}
	if got := p.requests()[0].header.Get("Authorization"); got != "" {
		t.Fatalf("Authorization = %q", got)
	}
	if strings.Contains(p.requests()[0].raw, `response_format`) {
		t.Fatal("json mode is off by default")
	}
}

func TestContentPartsAndMissingUsage(t *testing.T) {
	p := newProvider(t, func(_ int, w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"model": "m", "choices": []any{map[string]any{"finish_reason": "stop",
			"message": map[string]any{"content": []any{map[string]any{"type": "text", "text": "a"}, map[string]any{"type": "text", "text": "b"}}}}}})
	})
	c, _ := newClient(t, p, nil)
	r, err := c.Complete(context.Background(), hello)
	if err != nil || r.Text != "ab" {
		t.Fatalf("%+v %v", r, err)
	}
	if !r.Usage.Estimated || r.Usage.PromptTokens == 0 {
		t.Fatalf("usage = %+v: without a reported usage the budget still has to move", r.Usage)
	}
	if c.Totals().TokensLastDay == 0 {
		t.Fatal("estimated tokens were not counted")
	}
}

func TestBadRepliesAreErrors(t *testing.T) {
	for name, tc := range map[string]struct {
		reply func(int, http.ResponseWriter, *http.Request)
		want  error
		text  string
	}{
		"cut off by the output bound": {func(_ int, w http.ResponseWriter, _ *http.Request) { writeOK(w, `{"a":`, "length", 5, 5) }, ErrTruncated, ""},
		"empty":                       {func(_ int, w http.ResponseWriter, _ *http.Request) { writeOK(w, "  \n", "stop", 5, 0) }, ErrEmpty, ""},
		"not json":                    {func(_ int, w http.ResponseWriter, _ *http.Request) { _, _ = io.WriteString(w, "<html>gateway</html>") }, nil, "not JSON"},
		"no choices":                  {func(_ int, w http.ResponseWriter, _ *http.Request) { _, _ = io.WriteString(w, `{"choices":[]}`) }, nil, "no choices"},
		"error object with 200": {func(_ int, w http.ResponseWriter, _ *http.Request) {
			_, _ = io.WriteString(w, `{"error":{"message":"model overloaded","code":529}}`)
		}, nil, "overloaded"},
		"refusal": {func(_ int, w http.ResponseWriter, _ *http.Request) {
			_, _ = io.WriteString(w, `{"choices":[{"finish_reason":"stop","message":{"content":null,"refusal":"I cannot help with that"}}]}`)
		}, nil, "refused"},
	} {
		p := newProvider(t, tc.reply)
		c, _ := newClient(t, p, nil)
		_, err := c.Complete(context.Background(), hello)
		if err == nil {
			t.Errorf("%s: no error", name)
			continue
		}
		if tc.want != nil && !errors.Is(err, tc.want) {
			t.Errorf("%s: err = %v, want %v", name, err, tc.want)
		}
		if tc.text != "" && !strings.Contains(err.Error(), tc.text) {
			t.Errorf("%s: err = %v, want it to mention %q", name, err, tc.text)
		}
		if n := len(p.requests()); n != 1 {
			t.Errorf("%s: %d requests; an unusable reply is not retried", name, n)
		}
	}
}

func TestRetriesTransientFailuresAndHonoursRetryAfter(t *testing.T) {
	p := newProvider(t, func(n int, w http.ResponseWriter, r *http.Request) {
		switch n {
		case 1:
			w.Header().Set("Retry-After", "7")
			w.WriteHeader(429)
			_, _ = io.WriteString(w, `{"error":{"message":"slow down"}}`)
		case 2:
			w.WriteHeader(503)
		default:
			writeOK(w, "done", "stop", 3, 1)
		}
	})
	c, ck := newClient(t, p, nil)
	r, err := c.Complete(context.Background(), hello)
	if err != nil || r.Text != "done" || r.Attempts != 3 {
		t.Fatalf("%+v %v", r, err)
	}
	s := ck.recorded()
	if len(s) != 2 || s[0] != 7*time.Second || s[1] < 800*time.Millisecond || s[1] > 1200*time.Millisecond {
		t.Fatalf("sleeps = %v, want the 7s hint, then ~1s (500ms doubled, +-20%%)", s)
	}
	if n := c.Totals().Requests; n != 3 {
		t.Fatalf("each attempt counts: %d", n)
	}
}

func TestPermanentFailuresAreNotRetried(t *testing.T) {
	for _, status := range []int{400, 401, 403, 404} {
		p := newProvider(t, func(_ int, w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(status)
			_, _ = io.WriteString(w, `{"error":{"message":"no way","code":"nope"}}`)
		})
		c, _ := newClient(t, p, nil)
		_, err := c.Complete(context.Background(), hello)
		var le *Error
		if !errors.As(err, &le) || le.Status != status || le.Retryable || le.Code != "nope" || !strings.Contains(le.Error(), "no way") {
			t.Fatalf("status %d: err = %v", status, err)
		}
		if n := len(p.requests()); n != 1 {
			t.Fatalf("status %d: %d requests", status, n)
		}
	}
}

func TestGivesUpAfterTheConfiguredRetries(t *testing.T) {
	p := newProvider(t, func(_ int, w http.ResponseWriter, _ *http.Request) { w.WriteHeader(502) })
	c, _ := newClient(t, p, func(c *Config) { c.Retries = 2 })
	_, err := c.Complete(context.Background(), hello)
	var le *Error
	if !errors.As(err, &le) || le.Status != 502 {
		t.Fatalf("err = %v", err)
	}
	if n := len(p.requests()); n != 3 {
		t.Fatalf("%d requests, want 1 + 2 retries", n)
	}
	none, _ := newClient(t, p, func(c *Config) { c.Retries = 0 })
	before := len(p.requests())
	if _, err := none.Complete(context.Background(), hello); err == nil || len(p.requests()) != before+1 {
		t.Fatalf("retries: 0 must mean exactly one attempt (err %v, %d new requests)", err, len(p.requests())-before)
	}
}

func TestTransportFailuresAreRetriedAndCancellationStops(t *testing.T) {
	var calls atomic.Int32
	p := newProvider(t, func(n int, w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if n == 1 {
			panic(http.ErrAbortHandler) // the connection dies without an answer
		}
		writeOK(w, "after the reset", "stop", 1, 1)
	})
	c, _ := newClient(t, p, nil)
	r, err := c.Complete(context.Background(), hello)
	if err != nil || r.Text != "after the reset" || calls.Load() != 2 {
		t.Fatalf("%+v %v calls=%d", r, err, calls.Load())
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := c.Complete(ctx, hello); !errors.Is(err, context.Canceled) {
		t.Fatalf("a cancelled context must end the call: %v", err)
	}
}

func TestEachAttemptHasItsOwnTimeout(t *testing.T) {
	release := make(chan struct{})
	p := newProvider(t, func(n int, w http.ResponseWriter, r *http.Request) {
		if n == 1 {
			select {
			case <-release:
			case <-r.Context().Done():
			}
			return
		}
		writeOK(w, "second try", "stop", 1, 1)
	})
	t.Cleanup(func() { close(release) })
	c, _ := newClient(t, p, func(c *Config) { c.Timeout = 150 * time.Millisecond })
	r, err := c.Complete(context.Background(), hello)
	if err != nil || r.Text != "second try" {
		t.Fatalf("%+v %v", r, err)
	}
}

func TestBudgetsStopTheClientBeforeAnythingIsSent(t *testing.T) {
	p := newProvider(t, ok("x", 100, 50))
	c, ck := newClient(t, p, func(c *Config) { c.Budget = Budget{RequestsPerHour: 3, TokensPerDay: 400} })
	for i := 0; i < 2; i++ {
		if _, err := c.Complete(context.Background(), hello); err != nil {
			t.Fatal(err)
		}
	}
	// 300 tokens used, one request left: this one is allowed, and takes the tokens to 450
	if _, err := c.Complete(context.Background(), hello); err != nil {
		t.Fatal(err)
	}
	before := len(p.requests())
	_, err := c.Complete(context.Background(), hello)
	if !errors.Is(err, ErrBudget) || !strings.Contains(err.Error(), "requests in the last hour") {
		t.Fatalf("err = %v", err)
	}
	if len(p.requests()) != before {
		t.Fatal("a request was sent although the budget was used up")
	}
	// an hour later the request budget is free again, but the daily token budget (450 of 400) is not
	ck.advance(61 * time.Minute)
	_, err = c.Complete(context.Background(), hello)
	if !errors.Is(err, ErrBudget) || !strings.Contains(err.Error(), "tokens in the last 24 hours") {
		t.Fatalf("err = %v", err)
	}
	// a day later both are free
	ck.advance(24 * time.Hour)
	if _, err := c.Complete(context.Background(), hello); err != nil {
		t.Fatalf("after the windows passed: %v", err)
	}
	if tot := c.Totals(); tot.RequestsLastHour != 1 || tot.TokensLastDay != 150 {
		t.Fatalf("totals = %+v", tot)
	}
}

func TestOversizedInputIsRefusedLocally(t *testing.T) {
	p := newProvider(t, ok("x", 1, 1))
	c, _ := newClient(t, p, func(c *Config) { c.MaxInputBytes = 100 })
	_, err := c.Complete(context.Background(), Request{Messages: []Message{{Role: User, Content: strings.Repeat("a", 101)}}})
	if !errors.Is(err, ErrInputTooLarge) || len(p.requests()) != 0 {
		t.Fatalf("err = %v, %d requests", err, len(p.requests()))
	}
}

type recorder struct {
	mu   sync.Mutex
	rows []string
}

func (r *recorder) ObserveLLM(purpose, outcome string, p, c int, _ time.Duration) {
	r.mu.Lock()
	r.rows = append(r.rows, fmt.Sprintf("%s/%s/%d/%d", purpose, outcome, p, c))
	r.mu.Unlock()
}

func TestOutcomesAreObserved(t *testing.T) {
	obs := &recorder{}
	n := 0
	p := newProvider(t, func(_ int, w http.ResponseWriter, _ *http.Request) {
		n++
		if n == 2 {
			writeOK(w, "{", "length", 4, 4)
			return
		}
		writeOK(w, "fine", "stop", 4, 2)
	})
	c, _ := newClient(t, p, func(c *Config) { c.Observer = obs; c.Budget = Budget{RequestsPerHour: 3} })
	req := hello
	req.Purpose = "advisor"
	for i := 0; i < 4; i++ {
		_, _ = c.Complete(context.Background(), req)
	}
	got := strings.Join(obs.rows, " ")
	if got != "advisor/ok/4/2 advisor/truncated/4/4 advisor/ok/4/2 advisor/budget/0/0" {
		t.Fatalf("observed %q", got)
	}
	if f := c.Totals().Failures; f != 2 {
		t.Fatalf("failures = %d", f)
	}
}

// The key is a secret.Secret from the environment into the Authorization header and nowhere else.
func TestTheKeyNeverLeaks(t *testing.T) {
	p := newProvider(t, func(_ int, w http.ResponseWriter, r *http.Request) {
		// a careless provider that echoes the credential it was sent
		w.WriteHeader(401)
		_, _ = fmt.Fprintf(w, `{"error":{"message":"invalid key %s"}}`, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
	})
	var logs bytes.Buffer
	c, _ := newClient(t, p, func(c *Config) {
		c.Log = slog.New(slog.NewTextHandler(&logs, &slog.HandlerOptions{Level: slog.LevelDebug}))
	})
	_, err := c.Complete(context.Background(), hello)
	if err == nil {
		t.Fatal("expected a 401")
	}
	for name, text := range map[string]string{"error": err.Error(), "string": c.String(), "logs": logs.String(), "totals": fmt.Sprintf("%+v", c.Totals()), "%v of config": fmt.Sprintf("%v %+v", c.cfg, c.cfg)} {
		if strings.Contains(text, testKey) {
			t.Errorf("the key appears in the %s: %s", name, text)
		}
	}
	if !strings.Contains(err.Error(), secret.Redacted) {
		t.Fatalf("the echoed key should have been scrubbed from the provider's message: %v", err)
	}
}

func TestConcurrencyIsBounded(t *testing.T) {
	var cur, peak atomic.Int32
	p := newProvider(t, func(_ int, w http.ResponseWriter, _ *http.Request) {
		n := cur.Add(1)
		for {
			old := peak.Load()
			if n <= old || peak.CompareAndSwap(old, n) {
				break
			}
		}
		time.Sleep(40 * time.Millisecond)
		cur.Add(-1)
		writeOK(w, "x", "stop", 1, 1)
	})
	c, _ := newClient(t, p, func(c *Config) { c.Concurrency = 2 })
	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, err := c.Complete(context.Background(), hello); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	if peak.Load() > 2 {
		t.Fatalf("%d requests in flight, limit 2", peak.Load())
	}
}

func TestNewValidates(t *testing.T) {
	for name, cfg := range map[string]Config{
		"no url":     {Model: "m"},
		"bad scheme": {BaseURL: "ftp://x", Model: "m"},
		"no host":    {BaseURL: "http://", Model: "m"},
		"no model":   {BaseURL: "https://x/v1"},
	} {
		if _, err := New(cfg); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	c, err := New(Config{BaseURL: "https://host.example/api/v1/", Model: "m"})
	if err != nil || c.url != "https://host.example/api/v1/chat/completions" {
		t.Fatalf("%v %v", c, err)
	}
}
