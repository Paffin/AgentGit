// Package agwp is a small hand-written client for the HTTP/JSON transport of
// AGWP v1 (docs/protocol/agwp-v1.md, docs/protocol/openapi.json).
//
// It covers the operations the orchestrator uses and nothing more: bearer
// authentication, Idempotency-Key on mutations, Request-Timeout deadlines,
// retries only for errors the server marks retryable (honouring Retry-After),
// typed *Error values, and a Server-Sent Events reader that resumes from the
// last sequence number.
//
// Retry policy in one paragraph. A request that the server answered with an
// error whose "retryable" flag is set is repeated after the server's hint. A
// request that failed in transport (connection reset, timeout) is repeated
// only when repeating is provably safe: reads, and the few mutations for which
// the server records Idempotency-Keys (see Honoured). Every other mutation
// fails with an *OutcomeUnknownError and the caller reconciles by reading
// state.
package agwp

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// ProtocolVersion is the only protocol version this client speaks.
const ProtocolVersion = "agwp.v1"

// Observer receives client-side measurements (implemented by metrics.Service).
type Observer interface {
	ObserveRequest(identity, operation string)
	ObserveError(code string)
	ObserveRetry(code string)
}

type nopObserver struct{}

func (nopObserver) ObserveRequest(string, string) {}
func (nopObserver) ObserveError(string)           {}
func (nopObserver) ObserveRetry(string)           {}

// RetryPolicy controls retries of retryable failures.
type RetryPolicy struct {
	// MaxAttempts is the total number of tries (1 disables retries).
	MaxAttempts int
	// BaseDelay is the first backoff step; it doubles per attempt.
	BaseDelay time.Duration
	// MaxDelay caps the backoff.
	MaxDelay time.Duration
	// MaxRetryAfter caps how long a server hint is obeyed; a longer hint
	// aborts the call with the server's error instead of blocking.
	MaxRetryAfter time.Duration
}

// DefaultRetry is used unless overridden.
var DefaultRetry = RetryPolicy{MaxAttempts: 4, BaseDelay: 250 * time.Millisecond, MaxDelay: 5 * time.Second, MaxRetryAfter: 30 * time.Second}

// Client talks to one AGWP server with one credential. It is safe for
// concurrent use; derive variants with WithRun and WithToken.
type Client struct {
	base      *url.URL
	token     secret.Secret
	identity  string // metrics/log label, never a secret
	run       string
	hc        *http.Client
	stream    *http.Client
	clk       clock.Clock
	retry     RetryPolicy
	timeout   time.Duration
	obs       Observer
	userAgent string
	jitter    func() float64 // [0,1)
	maxBody   int64
}

// Option customises a Client.
type Option func(*Client)

// WithHTTPClient replaces the transport used for request/response calls.
func WithHTTPClient(hc *http.Client) Option { return func(c *Client) { c.hc = hc } }

// WithStreamClient replaces the transport used for SSE (it must have no overall timeout).
func WithStreamClient(hc *http.Client) Option { return func(c *Client) { c.stream = hc } }

// WithClock injects a clock (used for retry sleeps).
func WithClock(k clock.Clock) Option { return func(c *Client) { c.clk = k } }

// WithObserver installs a measurement hook.
func WithObserver(o Observer) Option { return func(c *Client) { c.obs = o } }

// WithRetry overrides the retry policy.
func WithRetry(p RetryPolicy) Option { return func(c *Client) { c.retry = p } }

// WithTimeout sets the per-request deadline (default 30s).
func WithTimeout(d time.Duration) Option { return func(c *Client) { c.timeout = d } }

// WithIdentityLabel names the credential in metrics and logs (e.g. "orchestrator").
func WithIdentityLabel(s string) Option { return func(c *Client) { c.identity = s } }

// WithUserAgent sets the User-Agent header.
func WithUserAgent(s string) Option { return func(c *Client) { c.userAgent = s } }

// WithJitter injects the randomness source for backoff jitter (tests).
func WithJitter(f func() float64) Option { return func(c *Client) { c.jitter = f } }

// WithMaxResponseBytes bounds how much of a response body is read.
func WithMaxResponseBytes(n int64) Option { return func(c *Client) { c.maxBody = n } }

// New builds a client for the server at baseURL authenticating with token.
func New(baseURL string, token secret.Secret, opts ...Option) (*Client, error) {
	u, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return nil, fmt.Errorf("agwp: invalid server URL %q (want http(s)://host[:port])", baseURL)
	}
	u.RawQuery, u.Fragment = "", ""
	c := &Client{
		base: u, token: token, identity: "client", clk: clock.Real{}, retry: DefaultRetry,
		timeout: 30 * time.Second, obs: nopObserver{}, userAgent: "agentgit-orchestrator",
		jitter: func() float64 { return 0.5 }, maxBody: 64 << 20,
	}
	for _, o := range opts {
		o(c)
	}
	if c.hc == nil {
		c.hc = &http.Client{Transport: newTransport(), CheckRedirect: noRedirect}
	}
	if c.stream == nil {
		c.stream = &http.Client{Transport: newTransport(), CheckRedirect: noRedirect}
	}
	if c.retry.MaxAttempts < 1 {
		c.retry.MaxAttempts = 1
	}
	return c, nil
}

func noRedirect(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }

func newTransport() *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.ResponseHeaderTimeout = 0 // per-request deadlines come from contexts
	t.MaxIdleConnsPerHost = 8
	return t
}

// BaseURL returns the server URL (without credentials).
func (c *Client) BaseURL() string { return c.base.String() }

// Identity returns the metrics label of this client's credential.
func (c *Client) Identity() string { return c.identity }

// WithRun returns a client that acts as the given agent run (ag-run header).
// Only an agent credential may do this.
func (c *Client) WithRun(runID string) *Client {
	cp := *c
	cp.run = runID
	return &cp
}

// WithToken returns a client with a different credential and label.
func (c *Client) WithToken(token secret.Secret, label string) *Client {
	cp := *c
	cp.token, cp.identity, cp.run = token, label, ""
	return &cp
}

// String never reveals the credential.
func (c *Client) String() string {
	return fmt.Sprintf("agwp.Client{%s as %s}", c.base, c.identity)
}

// Honoured lists the mutating operations for which the server is known to
// record Idempotency-Keys, so that repeating them after an unknown outcome is
// safe. The protocol document promises this for every mutation; the server
// (as of this writing) implements it for these only (see the server-gap list in
// the service README). When the server catches up, add operations here.
var Honoured = map[string]bool{
	"repo.create":           true,
	"proposal.create":       true,
	"integration.integrate": true,
	"objects.publish":       true,
}

type call struct {
	op     string // operationId, used for metrics and idempotency policy
	method string
	path   string
	query  url.Values
	body   any
	out    any
	// raw receives the body of binary endpoints instead of JSON decoding.
	raw     *[]byte
	rawMax  int64
	timeout time.Duration
}

func (c *Client) do(ctx context.Context, cl *call) error {
	mutating := cl.method != http.MethodGet && cl.method != http.MethodHead
	var key string
	if mutating {
		key = newKey()
	}
	safeToRepeat := !mutating || Honoured[cl.op]
	c.obs.ObserveRequest(c.identity, cl.op)
	var lastErr error
	for attempt := 1; ; attempt++ {
		err := c.once(ctx, cl, key)
		if err == nil {
			return nil
		}
		lastErr = err
		if ctx.Err() != nil {
			return ctx.Err()
		}
		var apiErr *Error
		switch {
		case errors.As(err, &apiErr):
			c.obs.ObserveError(apiErr.Code)
			if !apiErr.Retryable || attempt >= c.retry.MaxAttempts {
				return err
			}
			wait := c.backoff(attempt)
			if apiErr.RetryAfter > 0 {
				if c.retry.MaxRetryAfter > 0 && apiErr.RetryAfter > c.retry.MaxRetryAfter {
					return err
				}
				wait = max(wait, apiErr.RetryAfter)
			}
			if dl, ok := ctx.Deadline(); ok && c.clk.Now().Add(wait).After(dl) {
				return err
			}
			c.obs.ObserveRetry(apiErr.Code)
			if serr := clock.Sleep(ctx, c.clk, wait); serr != nil {
				return serr
			}
		case isTransport(err):
			c.obs.ObserveError("transport")
			if !safeToRepeat {
				return &OutcomeUnknownError{Operation: cl.op, Err: err}
			}
			if attempt >= c.retry.MaxAttempts {
				return err
			}
			c.obs.ObserveRetry("transport")
			if serr := clock.Sleep(ctx, c.clk, c.backoff(attempt)); serr != nil {
				return serr
			}
		default:
			return lastErr
		}
	}
}

// backoff is the exponential step for the given (1-based) attempt, with ±20% jitter.
func (c *Client) backoff(attempt int) time.Duration {
	d := float64(c.retry.BaseDelay) * math.Pow(2, float64(attempt-1))
	if limit := float64(c.retry.MaxDelay); limit > 0 && d > limit {
		d = limit
	}
	d *= 0.8 + 0.4*c.jitter()
	return time.Duration(d)
}

// transportError marks failures below the HTTP layer.
type transportError struct{ err error }

func (e *transportError) Error() string { return e.err.Error() }
func (e *transportError) Unwrap() error { return e.err }

func isTransport(err error) bool {
	var t *transportError
	return errors.As(err, &t)
}

func (c *Client) once(ctx context.Context, cl *call, key string) error {
	timeout := cl.timeout
	if timeout <= 0 {
		timeout = c.timeout
	}
	if dl, ok := ctx.Deadline(); ok {
		if rem := time.Until(dl); rem < timeout {
			timeout = rem
		}
	}
	if timeout <= 0 {
		return context.DeadlineExceeded
	}
	rctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	var body io.Reader
	if cl.body != nil {
		b, err := json.Marshal(cl.body)
		if err != nil {
			return fmt.Errorf("agwp: encode %s request: %w", cl.op, err)
		}
		body = bytes.NewReader(b)
	}
	u, err := c.urlFor(cl.path, cl.query)
	if err != nil {
		return fmt.Errorf("agwp: %s: %w", cl.op, err)
	}
	req, err := http.NewRequestWithContext(rctx, cl.method, u, body)
	if err != nil {
		return fmt.Errorf("agwp: build %s request: %w", cl.op, err)
	}
	c.decorate(req, key, timeout)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.hc.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return &transportError{err: scrubURL(err)}
	}
	defer resp.Body.Close()
	limit := c.maxBody
	if cl.rawMax > 0 {
		limit = cl.rawMax
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return &transportError{err: err}
	}
	if int64(len(data)) > limit {
		return fmt.Errorf("%w: %s response exceeds %d bytes", ErrResponseTooLarge, cl.op, limit)
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return parseError(resp, data)
	}
	if cl.raw != nil {
		*cl.raw = data
		return nil
	}
	if cl.out == nil || len(bytes.TrimSpace(data)) == 0 {
		return nil
	}
	if err := json.Unmarshal(data, cl.out); err != nil {
		return fmt.Errorf("agwp: decode %s response: %w", cl.op, err)
	}
	return nil
}

// urlFor joins the server's base URL with an already-escaped path. The escaped
// form is kept as RawPath so escaped characters are not escaped a second time.
func (c *Client) urlFor(escapedPath string, q url.Values) (string, error) {
	u := *c.base
	escaped := strings.TrimRight(c.base.EscapedPath(), "/") + escapedPath
	decoded, err := url.PathUnescape(escaped)
	if err != nil {
		return "", fmt.Errorf("bad path: %w", err)
	}
	u.Path, u.RawPath = decoded, escaped
	if len(q) > 0 {
		u.RawQuery = q.Encode()
	}
	return u.String(), nil
}

func (c *Client) decorate(req *http.Request, key string, timeout time.Duration) {
	req.Header.Set("Authorization", "Bearer "+c.token.Reveal())
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", c.userAgent)
	req.Header.Set("X-Request-Id", newRequestID())
	req.Header.Set("Request-Timeout", strconv.FormatInt(timeout.Milliseconds(), 10))
	if key != "" {
		req.Header.Set("Idempotency-Key", key)
	}
	if c.run != "" {
		req.Header.Set("Ag-Run", c.run)
	}
}

// scrubURL drops the request URL that *url.Error embeds, keeping the cause.
func scrubURL(err error) error {
	var ue *url.Error
	if errors.As(err, &ue) && ue.Err != nil {
		return ue.Err
	}
	return err
}

func parseError(resp *http.Response, data []byte) error {
	var env struct {
		Error struct {
			Code         string            `json:"code"`
			Message      string            `json:"message"`
			Retryable    bool              `json:"retryable"`
			RetryAfterMs int64             `json:"retry_after_ms"`
			RequestID    string            `json:"request_id"`
			Details      map[string]string `json:"details"`
		} `json:"error"`
	}
	e := &Error{HTTPStatus: resp.StatusCode, RequestID: resp.Header.Get("X-Request-Id")}
	if json.Unmarshal(data, &env) == nil && env.Error.Code != "" {
		e.Code, e.Message, e.Retryable = env.Error.Code, env.Error.Message, env.Error.Retryable
		e.Details = env.Error.Details
		if env.Error.RequestID != "" {
			e.RequestID = env.Error.RequestID
		}
		if env.Error.RetryAfterMs > 0 {
			e.RetryAfter = time.Duration(env.Error.RetryAfterMs) * time.Millisecond
		}
	} else {
		e.Code = codeForStatus(resp.StatusCode)
		e.Retryable = retryableStatus(resp.StatusCode)
		msg := strings.TrimSpace(string(data))
		if len(msg) > 200 {
			msg = msg[:200] + "..."
		}
		e.Message = msg
	}
	if e.RetryAfter == 0 {
		if s := resp.Header.Get("Retry-After"); s != "" {
			if secs, err := strconv.Atoi(strings.TrimSpace(s)); err == nil && secs >= 0 {
				e.RetryAfter = time.Duration(secs) * time.Second
			} else if t, err := http.ParseTime(s); err == nil {
				if d := time.Until(t); d > 0 {
					e.RetryAfter = d
				}
			}
		}
	}
	return e
}

func newKey() string { return "orch-" + randHex(16) }

func newRequestID() string { return "orch_" + randHex(10) }

func randHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		// crypto/rand failing means the platform is broken; a constant is
		// still a valid (if useless) correlation id, never a security value.
		return strings.Repeat("0", 2*n)
	}
	return hex.EncodeToString(b)
}

// repoPath renders "owner/name" as the /v1/repos/{owner}/{name} prefix.
func repoPath(repo string) (string, error) {
	owner, name, ok := strings.Cut(repo, "/")
	if !ok || owner == "" || name == "" || strings.Contains(name, "/") {
		return "", fmt.Errorf("agwp: repository %q must look like owner/name", repo)
	}
	return "/v1/repos/" + url.PathEscape(owner) + "/" + url.PathEscape(name), nil
}

// repoCall is the helper behind the repository-scoped API methods.
func (c *Client) repoCall(ctx context.Context, op, method, repo, suffix string, q url.Values, body, out any) error {
	p, err := repoPath(repo)
	if err != nil {
		return err
	}
	return c.do(ctx, &call{op: op, method: method, path: p + suffix, query: q, body: body, out: out})
}
