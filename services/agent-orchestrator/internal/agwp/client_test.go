package agwp

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
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

// instantClock never really sleeps; it records what it was asked to sleep.
type instantClock struct {
	mu    sync.Mutex
	slept []time.Duration
}

func (c *instantClock) Now() time.Time { return time.Unix(1_000_000, 0) }
func (c *instantClock) NewTimer(d time.Duration) clock.Timer {
	c.mu.Lock()
	c.slept = append(c.slept, d)
	c.mu.Unlock()
	ch := make(chan time.Time, 1)
	ch <- c.Now()
	return instantTimer{ch}
}
func (c *instantClock) NewTicker(time.Duration) clock.Ticker { panic("unused") }
func (c *instantClock) sleeps() []time.Duration {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]time.Duration(nil), c.slept...)
}

type instantTimer struct{ ch chan time.Time }

func (t instantTimer) C() <-chan time.Time { return t.ch }
func (t instantTimer) Stop() bool          { return true }

type recObserver struct {
	mu                        sync.Mutex
	requests, errors, retries []string
}

func (r *recObserver) ObserveRequest(id, op string) {
	r.mu.Lock()
	r.requests = append(r.requests, id+":"+op)
	r.mu.Unlock()
}
func (r *recObserver) ObserveError(code string) {
	r.mu.Lock()
	r.errors = append(r.errors, code)
	r.mu.Unlock()
}
func (r *recObserver) ObserveRetry(code string) {
	r.mu.Lock()
	r.retries = append(r.retries, code)
	r.mu.Unlock()
}

const testToken = "agt_cred123_SUPERSECRETVALUE0123456789"

func newTestClient(t *testing.T, h http.Handler, opts ...Option) (*Client, *instantClock, *recObserver) {
	t.Helper()
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	ck := &instantClock{}
	obs := &recObserver{}
	base := []Option{WithClock(ck), WithObserver(obs), WithIdentityLabel("test")}
	c, err := New(srv.URL, secret.New(testToken), append(base, opts...)...)
	if err != nil {
		t.Fatal(err)
	}
	return c, ck, obs
}

func apiError(w http.ResponseWriter, status int, code string, retryable bool, retryAfterMs int64) {
	w.Header().Set("Content-Type", "application/json")
	if retryAfterMs > 0 {
		w.Header().Set("Retry-After", fmt.Sprint((retryAfterMs+999)/1000))
	}
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{
		"code": code, "message": "nope: " + code, "retryable": retryable, "retry_after_ms": retryAfterMs,
		"request_id": "req_srv", "details": map[string]string{"stream": "main"},
	}})
}

func TestRequestCarriesCredentialsAndProtocolHeaders(t *testing.T) {
	var got http.Header
	var gotPath, gotQuery string
	c, _, obs := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, gotPath, gotQuery = r.Header.Clone(), r.URL.Path, r.URL.RawQuery
		_ = json.NewEncoder(w).Encode(map[string]any{"tasks": []any{}})
	}), WithTimeout(7*time.Second))
	if _, err := c.WithRun("run_9").ListTasks(context.Background(), "acme/web", "open", true); err != nil {
		t.Fatal(err)
	}
	if got.Get("Authorization") != "Bearer "+testToken {
		t.Errorf("Authorization = %q", got.Get("Authorization"))
	}
	if got.Get("Ag-Run") != "run_9" {
		t.Errorf("Ag-Run = %q", got.Get("Ag-Run"))
	}
	if ms := got.Get("Request-Timeout"); ms == "" || ms == "0" {
		t.Errorf("Request-Timeout = %q", ms)
	}
	if !strings.HasPrefix(got.Get("X-Request-Id"), "orch_") {
		t.Errorf("X-Request-Id = %q", got.Get("X-Request-Id"))
	}
	if got.Get("Idempotency-Key") != "" {
		t.Errorf("reads must not carry an Idempotency-Key, got %q", got.Get("Idempotency-Key"))
	}
	if gotPath != "/v1/repos/acme/web/tasks" || gotQuery != "ready_only=true&state=open" {
		t.Errorf("request = %s?%s", gotPath, gotQuery)
	}
	if len(obs.requests) != 1 || obs.requests[0] != "test:task.list" {
		t.Errorf("observer saw %v", obs.requests)
	}
}

func TestRequestTimeoutHonoursContextDeadline(t *testing.T) {
	var ms string
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ms = r.Header.Get("Request-Timeout")
		_, _ = io.WriteString(w, `{}`)
	}))
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if _, err := c.Whoami(ctx); err != nil {
		t.Fatal(err)
	}
	var n int
	_, _ = fmt.Sscan(ms, &n)
	if n <= 0 || n > 2000 {
		t.Fatalf("Request-Timeout = %q, want <= 2000", ms)
	}
}

func TestMutationsCarryAnIdempotencyKeyStableAcrossRetries(t *testing.T) {
	var keys []string
	var calls atomic.Int32
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		keys = append(keys, r.Header.Get("Idempotency-Key"))
		if calls.Add(1) == 1 {
			apiError(w, 429, CodeRateLimited, true, 100)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"run_id": "run_1"})
	}))
	run, err := c.StartRun(context.Background(), "acme/web", "bot", "task_1")
	if err != nil {
		t.Fatal(err)
	}
	if run.RunID != "run_1" || len(keys) != 2 || keys[0] == "" || keys[0] != keys[1] {
		t.Fatalf("run=%+v keys=%v", run, keys)
	}
}

func TestErrorsAreTyped(t *testing.T) {
	c, _, obs := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		apiError(w, 404, CodeNotFound, false, 0)
	}))
	_, err := c.GetTask(context.Background(), "acme/web", "task_x")
	e, ok := AsError(err)
	if !ok {
		t.Fatalf("not an *Error: %v", err)
	}
	if e.Code != CodeNotFound || e.HTTPStatus != 404 || e.Retryable || e.RequestID != "req_srv" || e.Details["stream"] != "main" {
		t.Fatalf("error = %+v", e)
	}
	if !IsNotFound(err) || !IsCode(err, CodeAborted, CodeNotFound) || IsRetryable(err) {
		t.Fatal("helper predicates disagree with the error")
	}
	if !strings.Contains(err.Error(), "not_found") || !strings.Contains(err.Error(), "req_srv") {
		t.Fatalf("Error() = %q", err.Error())
	}
	if len(obs.errors) != 1 || obs.errors[0] != CodeNotFound {
		t.Fatalf("observed errors = %v", obs.errors)
	}
}

func TestNonAGWPErrorBodiesAreMappedFromTheStatus(t *testing.T) {
	tests := []struct {
		status    int
		body      string
		code      string
		retryable bool
	}{
		{502, "<html>bad gateway</html>", CodeUnavailable, true},
		{503, "", CodeUnavailable, true},
		{504, "timeout", CodeDeadlineExceeded, false},
		{401, "nope", CodeUnauthenticated, false},
		{500, "boom", CodeInternal, false},
	}
	for _, tt := range tests {
		c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(tt.status)
			_, _ = io.WriteString(w, tt.body)
		}), WithRetry(RetryPolicy{MaxAttempts: 1}))
		_, err := c.Whoami(context.Background())
		e, ok := AsError(err)
		if !ok || e.Code != tt.code || e.Retryable != tt.retryable || e.HTTPStatus != tt.status {
			t.Errorf("status %d: got %+v (%v)", tt.status, e, err)
		}
	}
}

func TestRetryAfterIsHonouredAndBackoffGrows(t *testing.T) {
	var calls atomic.Int32
	c, ck, obs := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch calls.Add(1) {
		case 1:
			apiError(w, 429, CodeRateLimited, true, 3000)
		case 2:
			apiError(w, 503, CodeUnavailable, true, 0)
		default:
			_, _ = io.WriteString(w, `{"identity":"service:orchestrator"}`)
		}
	}), WithRetry(RetryPolicy{MaxAttempts: 4, BaseDelay: 100 * time.Millisecond, MaxDelay: time.Second, MaxRetryAfter: 10 * time.Second}))
	p, err := c.Whoami(context.Background())
	if err != nil || p.Identity != "service:orchestrator" {
		t.Fatalf("p=%+v err=%v", p, err)
	}
	s := ck.sleeps()
	if len(s) != 2 || s[0] != 3*time.Second {
		t.Fatalf("sleeps = %v, want the 3s hint first", s)
	}
	if s[1] < 160*time.Millisecond || s[1] > 240*time.Millisecond { // base*2 with +-20% jitter
		t.Fatalf("second sleep = %v, want ~200ms", s[1])
	}
	if len(obs.retries) != 2 || obs.retries[0] != CodeRateLimited {
		t.Fatalf("retries = %v", obs.retries)
	}
}

func TestRetryAfterBeyondTheCapFailsFast(t *testing.T) {
	var calls atomic.Int32
	c, ck, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		apiError(w, 429, CodeQuotaExceeded, true, 120_000)
	}), WithRetry(RetryPolicy{MaxAttempts: 5, BaseDelay: time.Millisecond, MaxRetryAfter: 5 * time.Second}))
	_, err := c.Whoami(context.Background())
	if !IsCode(err, CodeQuotaExceeded) || calls.Load() != 1 || len(ck.sleeps()) != 0 {
		t.Fatalf("err=%v calls=%d sleeps=%v", err, calls.Load(), ck.sleeps())
	}
}

func TestNonRetryableErrorsAreNotRetried(t *testing.T) {
	var calls atomic.Int32
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		apiError(w, 403, CodePermissionDenied, false, 0)
	}))
	if _, err := c.Whoami(context.Background()); !IsCode(err, CodePermissionDenied) || calls.Load() != 1 {
		t.Fatalf("err=%v calls=%d", err, calls.Load())
	}
}

func TestRetriesStopAtMaxAttempts(t *testing.T) {
	var calls atomic.Int32
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		apiError(w, 503, CodeUnavailable, true, 0)
	}), WithRetry(RetryPolicy{MaxAttempts: 3, BaseDelay: time.Millisecond}))
	if _, err := c.Whoami(context.Background()); !IsCode(err, CodeUnavailable) || calls.Load() != 3 {
		t.Fatalf("err=%v calls=%d", err, calls.Load())
	}
}

// dropConn answers n requests by closing the connection without a response.
func dropConn(n int32, then http.HandlerFunc) http.HandlerFunc {
	var calls atomic.Int32
	return func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) <= n {
			conn, _, _ := w.(http.Hijacker).Hijack()
			conn.Close()
			return
		}
		then(w, r)
	}
}

func TestTransportFailuresAreRetriedForReads(t *testing.T) {
	c, _, obs := newTestClient(t, dropConn(2, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"identity":"user:a"}`)
	}))
	p, err := c.Whoami(context.Background())
	if err != nil || p.Identity != "user:a" {
		t.Fatalf("p=%+v err=%v", p, err)
	}
	if len(obs.retries) != 2 || obs.retries[0] != "transport" {
		t.Fatalf("retries = %v", obs.retries)
	}
}

func TestTransportFailureOfAnUnkeyedMutationIsAnUnknownOutcome(t *testing.T) {
	var calls atomic.Int32
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		conn, _, _ := w.(http.Hijacker).Hijack()
		conn.Close()
	}))
	_, err := c.MintCredential(context.Background(), MintRequest{Subject: "agent:bot", TTLSecs: 60})
	if !errors.Is(err, ErrOutcomeUnknown) {
		t.Fatalf("err = %v, want ErrOutcomeUnknown", err)
	}
	var ou *OutcomeUnknownError
	if !errors.As(err, &ou) || ou.Operation != "admin.credentials.mint" {
		t.Fatalf("err = %#v", err)
	}
	if calls.Load() != 1 {
		t.Fatalf("a mutation without a server-honoured key must not be repeated blindly (calls=%d)", calls.Load())
	}
}

func TestKeyedMutationsAreRepeatedWithTheSameKey(t *testing.T) {
	var mu sync.Mutex
	var keys []string
	c, _, _ := newTestClient(t, dropConn(1, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"id":"prop_1","state":"open"}`)
	}))
	// Wrap: record keys of every attempt, including the dropped one.
	inner := c.hc.Transport
	c.hc.Transport = roundTripFunc(func(r *http.Request) (*http.Response, error) {
		mu.Lock()
		keys = append(keys, r.Header.Get("Idempotency-Key"))
		mu.Unlock()
		return inner.RoundTrip(r)
	})
	p, err := c.CreateProposal(context.Background(), "acme/web", ProposalInput{Stream: "main", Head: "abc", Title: "t"})
	if err != nil || p.ID != "prop_1" {
		t.Fatalf("p=%+v err=%v", p, err)
	}
	if len(keys) != 2 || keys[0] == "" || keys[0] != keys[1] {
		t.Fatalf("keys = %v", keys)
	}
}

// The server records Idempotency-Keys for these mutations, so a lost response is
// repeated with the same key instead of being reported as an unknown outcome.
func TestMutationsTheServerHonoursKeysForAreRepeated(t *testing.T) {
	cases := map[string]func(c *Client) error{
		"task.set_attempt_limit": func(c *Client) error {
			_, err := c.SetTaskAttemptLimit(context.Background(), "acme/web", "task", 3, 2)
			return err
		},
		"task.create": func(c *Client) error {
			_, err := c.CreateTask(context.Background(), "acme/web", "t", "", nil, nil)
			return err
		},
		"agent.start_run": func(c *Client) error { _, err := c.StartRun(context.Background(), "acme/web", "bot", ""); return err },
		"lease.acquire": func(c *Client) error {
			_, err := c.AcquireLease(context.Background(), "acme/web", "src/**", false, time.Minute, "")
			return err
		},
	}
	for op, do := range cases {
		t.Run(op, func(t *testing.T) {
			var mu sync.Mutex
			var keys []string
			c, _, _ := newTestClient(t, dropConn(1, func(w http.ResponseWriter, r *http.Request) {
				_, _ = io.WriteString(w, `{"id":"x","run_id":"run_1","lease":{"id":"l"}}`)
			}))
			inner := c.hc.Transport
			c.hc.Transport = roundTripFunc(func(r *http.Request) (*http.Response, error) {
				mu.Lock()
				keys = append(keys, r.Header.Get("Idempotency-Key"))
				mu.Unlock()
				return inner.RoundTrip(r)
			})
			if err := do(c); err != nil {
				t.Fatalf("%s: %v", op, err)
			}
			if len(keys) != 2 || keys[0] == "" || keys[0] != keys[1] {
				t.Fatalf("%s: keys = %v", op, keys)
			}
		})
	}
}

func TestRateLimitedIsWaitedOutEvenWithoutTheRetryableFlag(t *testing.T) {
	var calls atomic.Int32
	c, _, obs := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			apiError(w, 429, CodeRateLimited, false, 1) // 1 ms
			return
		}
		_, _ = io.WriteString(w, `{"identity":"user:a"}`)
	}), WithRetry(RetryPolicy{MaxAttempts: 3, BaseDelay: time.Millisecond, MaxRetryAfter: time.Second}))
	if p, err := c.Whoami(context.Background()); err != nil || p.Identity != "user:a" {
		t.Fatalf("p=%+v err=%v", p, err)
	}
	if calls.Load() != 2 || len(obs.retries) != 1 || obs.retries[0] != CodeRateLimited {
		t.Fatalf("calls=%d retries=%v", calls.Load(), obs.retries)
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestContextCancellationWinsOverRetries(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		<-r.Context().Done()
	}))
	ctx, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(30 * time.Millisecond); cancel() }()
	_, err := c.Whoami(ctx)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
}

func TestSecretNeverAppearsInErrorsOrStrings(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, _, _ := w.(http.Hijacker).Hijack()
		conn.Close()
	}), WithRetry(RetryPolicy{MaxAttempts: 1}))
	_, err := c.Whoami(context.Background())
	if err == nil || strings.Contains(err.Error(), "SUPERSECRET") {
		t.Fatalf("err = %v", err)
	}
	for _, s := range []string{c.String(), fmt.Sprintf("%v", c), fmt.Sprintf("%+v", c.token)} {
		if strings.Contains(s, "SUPERSECRET") {
			t.Fatalf("leaked in %q", s)
		}
	}
}

func TestRedirectsAreNotFollowed(t *testing.T) {
	var leaked atomic.Bool
	other := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "" {
			leaked.Store(true)
		}
	}))
	defer other.Close()
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, other.URL, http.StatusTemporaryRedirect)
	}), WithRetry(RetryPolicy{MaxAttempts: 1}))
	if _, err := c.Whoami(context.Background()); err == nil {
		t.Fatal("a redirect must surface as an error")
	}
	if leaked.Load() {
		t.Fatal("the credential followed a redirect")
	}
}

func TestOversizedResponsesAreRejected(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(strings.Repeat("x", 5000)))
	}), WithMaxResponseBytes(1000))
	if _, err := c.GetFile(context.Background(), "acme/web", "main", "big.bin", 0); err == nil || !strings.Contains(err.Error(), "exceeds") {
		t.Fatalf("err = %v", err)
	}
	_, err := c.GetFile(context.Background(), "acme/web", "main", "big.bin", 100)
	if err == nil {
		t.Fatal("per-call cap must apply")
	}
}

func TestNilListsAreSentAsEmptyArrays(t *testing.T) {
	var body string
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		body = string(b)
		_, _ = io.WriteString(w, `{"credential":{"id":"cred_1"},"token":"agt_cred_1_secretsecretsecret"}`)
	}))
	m, err := c.MintCredential(context.Background(), MintRequest{Subject: "agent:bot", TTLSecs: 60, Scope: CredentialScope{Repos: []string{"acme/web"}}})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(body, "null") {
		t.Fatalf("body contains null: %s", body)
	}
	if !strings.Contains(body, `"operations":[]`) || !strings.Contains(body, `"streams":[]`) {
		t.Fatalf("body = %s", body)
	}
	if m.Token.Reveal() != "agt_cred_1_secretsecretsecret" || strings.Contains(fmt.Sprint(m), "secretsecret") {
		t.Fatalf("token handling wrong: %v", m)
	}
}

func TestRepoNamesAreValidated(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Error("no request should be made for an invalid repository")
	}))
	for _, bad := range []string{"", "web", "a/b/c", "/x", "x/"} {
		if _, err := c.ListTasks(context.Background(), bad, "", false); err == nil {
			t.Errorf("repo %q accepted", bad)
		}
	}
}

func TestNegotiateChecksTheProtocolVersion(t *testing.T) {
	version := "agwp.v1"
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"protocol_version": version, "features": []string{"events.replay"}})
	}))
	caps, err := c.Negotiate(context.Background())
	if err != nil || !caps.Has("events.replay") || caps.Has("nope") {
		t.Fatalf("caps=%+v err=%v", caps, err)
	}
	version = "agwp.v2"
	if _, err := c.Negotiate(context.Background()); err == nil {
		t.Fatal("a different protocol version must be refused")
	}
}

func TestPaginationIsFollowed(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/repos/acme/web/proposals":
			if r.URL.Query().Get("page_token") == "" {
				_, _ = io.WriteString(w, `{"proposals":[{"id":"p1"}],"next_page_token":"t2"}`)
			} else {
				_, _ = io.WriteString(w, `{"proposals":[{"id":"p2"}],"next_page_token":""}`)
			}
		case "/v1/repos/acme/web/tree":
			if r.URL.Query().Get("page_token") == "" {
				_, _ = io.WriteString(w, `{"tree_id":"t","entries":[{"path":"a"}],"next_page_token":"n"}`)
			} else {
				_, _ = io.WriteString(w, `{"tree_id":"t","entries":[{"path":"b"}],"next_page_token":""}`)
			}
		}
	}))
	ps, err := c.ListProposals(context.Background(), "acme/web", ProposalFilter{State: "open"})
	if err != nil || len(ps) != 2 || ps[1].ID != "p2" {
		t.Fatalf("ps=%+v err=%v", ps, err)
	}
	tr, err := c.GetTree(context.Background(), "acme/web", "main")
	if err != nil || len(tr.Entries) != 2 || tr.Entries[1].Path != "b" {
		t.Fatalf("tree=%+v err=%v", tr, err)
	}
}

func TestFilePathsAreEscapedPerSegment(t *testing.T) {
	var raw string
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		raw = r.URL.EscapedPath()
		_, _ = io.WriteString(w, "content")
	}))
	b, err := c.GetFile(context.Background(), "acme/web", "main", "dir with space/üñí #1.txt", 0)
	if err != nil || string(b) != "content" {
		t.Fatalf("b=%q err=%v", b, err)
	}
	if !strings.HasPrefix(raw, "/v1/repos/acme/web/files/dir%20with%20space/") || strings.Contains(raw, "#") {
		t.Fatalf("path = %q", raw)
	}
}

func TestWithTokenDropsTheRun(t *testing.T) {
	c, _, _ := newTestClient(t, http.NotFoundHandler())
	r := c.WithRun("run_1")
	o := r.WithToken(secret.New("other-token-0123456789"), "other")
	if o.run != "" || o.identity != "other" || r.run != "run_1" || c.run != "" {
		t.Fatalf("derived clients leak state: %+v %+v", r, o)
	}
}

func TestInvalidBaseURLs(t *testing.T) {
	for _, u := range []string{"", "localhost:8080", "ftp://x", "http://"} {
		if _, err := New(u, secret.New("x")); err == nil {
			t.Errorf("New(%q) should fail", u)
		}
	}
	if c, err := New("http://127.0.0.1:8080/", secret.New("x")); err != nil || c.BaseURL() != "http://127.0.0.1:8080" {
		t.Fatalf("c=%v err=%v", c, err)
	}
}

// A default client jitters its retries and reconnects (+-20%); a constant would send every client of a restarted
// server back in the same instant.
func TestDefaultBackoffIsJitteredWithinTwentyPercent(t *testing.T) {
	c, err := New("http://127.0.0.1:1", secret.New(testToken), WithRetry(RetryPolicy{MaxAttempts: 4, BaseDelay: time.Second, MaxDelay: time.Minute}))
	if err != nil {
		t.Fatal(err)
	}
	seen := map[time.Duration]bool{}
	for i := 0; i < 200; i++ {
		d := c.backoff(2) // 2s before jitter
		if d < 1600*time.Millisecond || d > 2400*time.Millisecond {
			t.Fatalf("backoff %v outside 2s +-20%%", d)
		}
		seen[d] = true
	}
	if len(seen) < 20 {
		t.Fatalf("only %d distinct delays in 200 draws: the jitter is not random", len(seen))
	}
	// an injected constant still works (tests rely on it)
	fixed, _ := New("http://127.0.0.1:1", secret.New(testToken), WithRetry(RetryPolicy{BaseDelay: time.Second, MaxDelay: time.Minute}), WithJitter(func() float64 { return 0.5 }))
	if d := fixed.backoff(2); d != 2*time.Second {
		t.Fatalf("fixed jitter: %v", d)
	}
}

func TestResponseBodyBoundIsConfigurable(t *testing.T) {
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"identity":"`+strings.Repeat("x", 4096)+`"}`)
	}), WithMaxResponseBytes(1024))
	if _, err := c.Whoami(context.Background()); err == nil {
		t.Fatal("a response over the configured bound was accepted")
	}
}
