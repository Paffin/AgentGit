// Package llm is a small client for OpenAI-compatible chat-completion APIs (POST {base_url}/chat/completions).
//
// It knows nothing about any provider: the base URL, the model, the name of the environment variable that holds the
// API key, the timeouts, the retries and the budgets are all configuration. Nothing here is hardcoded to a vendor, and
// the key is only ever a secret.Secret: it is placed in the Authorization header and nowhere else (not in errors, not
// in logs, not in the status output). With no key configured no Authorization header is sent, which is what a gateway
// that injects credentials itself (a corporate proxy, a sandbox egress proxy, a local server) needs.
//
// What a model says is never trusted here: Complete returns text, and the callers decode it into the AGWP-shaped
// types of their own package with validation (see json.go). A model can only PROPOSE: nothing in this package, or in
// the components that use it, integrates, approves or applies anything.
package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
	mrand "math/rand/v2"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// Role of a chat message.
const (
	System    = "system"
	User      = "user"
	Assistant = "assistant"
)

// Message is one chat message.
type Message struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// Request is one completion.
type Request struct {
	// Purpose names the component asking ("advisor", "review", "index"); it labels metrics and logs.
	Purpose  string
	Messages []Message
	// MaxOutputTokens overrides the configured bound (0: the configured one).
	MaxOutputTokens int
}

// Usage is what a completion cost, as reported by the provider (or estimated when the provider does not say).
type Usage struct {
	PromptTokens     int  `json:"prompt_tokens"`
	CompletionTokens int  `json:"completion_tokens"`
	Estimated        bool `json:"estimated,omitempty"`
}

// Response is a completion.
type Response struct {
	Text         string
	Model        string // the model the provider says answered (it may differ from the configured alias)
	FinishReason string
	RequestID    string
	Usage        Usage
	Attempts     int
	Duration     time.Duration
}

// Completer is what the components depend on (satisfied by *Client).
type Completer interface {
	// Complete asks the model. It never returns a partial answer: a reply cut off by the output bound is an error.
	Complete(ctx context.Context, req Request) (*Response, error)
	// Model is the configured model identifier.
	Model() string
}

// Observer receives measurements (implemented by metrics.Service).
type Observer interface {
	// ObserveLLM is called once per Complete: outcome is "ok", "budget", "error", "invalid" or "truncated".
	ObserveLLM(purpose, outcome string, promptTokens, completionTokens int, took time.Duration)
}

// Budget limits what the service may spend. Zero means no limit for that dimension. The counters live in memory and
// restart with the process: they are a guard rail against a loop gone wrong, not an accounting system.
type Budget struct {
	RequestsPerHour int
	TokensPerDay    int
}

// Config configures a Client.
type Config struct {
	// BaseURL is the API root, for example "https://host/api/v1"; "/chat/completions" is appended.
	BaseURL string
	// Model is sent as the "model" field.
	Model string
	// APIKey is optional; when empty no Authorization header is sent.
	APIKey secret.Secret
	// Headers are extra non-secret request headers (some gateways want an application name).
	Headers map[string]string
	// Timeout bounds one HTTP attempt (default 60s).
	Timeout time.Duration
	// Retries is the number of retries after the first attempt for retryable failures (0: none).
	Retries int
	// RetryBase is the first backoff step (default 500ms); it doubles, capped at RetryMax (default 10s).
	RetryBase, RetryMax time.Duration
	// MaxOutputTokens bounds the reply (default 1024). Temperature is sent as given (0 is deterministic-ish).
	MaxOutputTokens int
	Temperature     float64
	// MaxInputBytes bounds the size of everything sent in one request (default 120000); larger requests are refused
	// before anything leaves the machine.
	MaxInputBytes int
	// JSONMode asks the provider for a JSON object reply (response_format); not every model supports it.
	JSONMode bool
	// Concurrency bounds simultaneous requests (default 2).
	Concurrency int
	Budget      Budget

	HTTPClient *http.Client
	Clock      clock.Clock
	Observer   Observer
	Log        *slog.Logger
}

// Error is a failure reported by the API or by the transport.
type Error struct {
	Status    int // HTTP status; 0 for transport failures
	Code      string
	Message   string
	Retryable bool
	// RetryAfter is the provider's hint, if any.
	RetryAfter time.Duration
}

func (e *Error) Error() string {
	if e.Status == 0 {
		return "llm: " + e.Message
	}
	msg := e.Message
	if msg == "" {
		msg = http.StatusText(e.Status)
	}
	return fmt.Sprintf("llm: HTTP %d: %s", e.Status, msg)
}

// Sentinel errors.
var (
	// ErrBudget: a configured budget is used up; nothing was sent.
	ErrBudget = errors.New("llm: budget exhausted")
	// ErrInputTooLarge: the request is larger than the configured input bound; nothing was sent.
	ErrInputTooLarge = errors.New("llm: request exceeds the input bound")
	// ErrTruncated: the reply was cut off by the output bound.
	ErrTruncated = errors.New("llm: the reply was cut off by the output bound")
	// ErrEmpty: the model answered with nothing.
	ErrEmpty = errors.New("llm: empty reply")
)

// Client talks to one endpoint with one model. It is safe for concurrent use.
type Client struct {
	cfg  Config
	url  string
	hc   *http.Client
	clk  clock.Clock
	log  *slog.Logger
	sem  chan struct{}
	obs  Observer
	jit  func() float64
	mu   sync.Mutex
	reqs []time.Time // attempts in the last hour
	toks []tokenUse  // usage in the last day
	tot  Totals
}

type tokenUse struct {
	at time.Time
	n  int
}

// Totals is a snapshot for the status output.
type Totals struct {
	Requests         uint64 `json:"requests"`
	Failures         uint64 `json:"failures"`
	PromptTokens     uint64 `json:"prompt_tokens"`
	CompletionTokens uint64 `json:"completion_tokens"`
	// Requests in the last hour and tokens in the last 24 hours, against the budget.
	RequestsLastHour int `json:"requests_last_hour"`
	TokensLastDay    int `json:"tokens_last_day"`
}

// New validates the configuration and builds a client.
func New(cfg Config) (*Client, error) {
	u, err := url.Parse(strings.TrimRight(cfg.BaseURL, "/"))
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return nil, fmt.Errorf("llm: invalid base URL %q (want http(s)://host[/path])", cfg.BaseURL)
	}
	if strings.TrimSpace(cfg.Model) == "" {
		return nil, errors.New("llm: a model is required")
	}
	if cfg.Timeout <= 0 {
		cfg.Timeout = 60 * time.Second
	}
	if cfg.Retries < 0 {
		cfg.Retries = 0 // the number of retries after the first attempt; the configuration layer supplies the default
	}
	if cfg.RetryBase <= 0 {
		cfg.RetryBase = 500 * time.Millisecond
	}
	if cfg.RetryMax <= 0 {
		cfg.RetryMax = 10 * time.Second
	}
	if cfg.MaxOutputTokens <= 0 {
		cfg.MaxOutputTokens = 1024
	}
	if cfg.MaxInputBytes <= 0 {
		cfg.MaxInputBytes = 120_000
	}
	if cfg.Concurrency <= 0 {
		cfg.Concurrency = 2
	}
	c := &Client{cfg: cfg, url: u.String() + "/chat/completions", hc: cfg.HTTPClient, clk: cfg.Clock, log: cfg.Log, obs: cfg.Observer, jit: mrand.Float64,
		sem: make(chan struct{}, cfg.Concurrency)}
	if c.hc == nil {
		c.hc = &http.Client{Transport: defaultTransport()}
	}
	if c.clk == nil {
		c.clk = clock.Real{}
	}
	if c.log == nil {
		c.log = slog.Default()
	}
	return c, nil
}

func defaultTransport() *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone() // honours HTTPS_PROXY and the system trust store
	t.MaxIdleConnsPerHost = 4
	return t
}

// Model implements Completer.
func (c *Client) Model() string { return c.cfg.Model }

// String never reveals the key.
func (c *Client) String() string { return fmt.Sprintf("llm.Client{%s model=%s}", c.url, c.cfg.Model) }

// Totals returns the usage counters.
func (c *Client) Totals() Totals {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.clk.Now()
	c.pruneLocked(now)
	t := c.tot
	t.RequestsLastHour = len(c.reqs)
	for _, u := range c.toks {
		t.TokensLastDay += u.n
	}
	return t
}

func (c *Client) pruneLocked(now time.Time) {
	hour, day := now.Add(-time.Hour), now.Add(-24*time.Hour)
	i := 0
	for i < len(c.reqs) && c.reqs[i].Before(hour) {
		i++
	}
	c.reqs = c.reqs[i:]
	j := 0
	for j < len(c.toks) && c.toks[j].at.Before(day) {
		j++
	}
	c.toks = c.toks[j:]
}

// reserve checks the budget and counts one attempt.
func (c *Client) reserve() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.clk.Now()
	c.pruneLocked(now)
	if b := c.cfg.Budget.RequestsPerHour; b > 0 && len(c.reqs) >= b {
		return fmt.Errorf("%w: %d requests in the last hour (limit %d)", ErrBudget, len(c.reqs), b)
	}
	if b := c.cfg.Budget.TokensPerDay; b > 0 {
		used := 0
		for _, u := range c.toks {
			used += u.n
		}
		if used >= b {
			return fmt.Errorf("%w: %d tokens in the last 24 hours (limit %d)", ErrBudget, used, b)
		}
	}
	c.reqs = append(c.reqs, now)
	c.tot.Requests++
	return nil
}

func (c *Client) record(u Usage) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.toks = append(c.toks, tokenUse{at: c.clk.Now(), n: u.PromptTokens + u.CompletionTokens})
	c.tot.PromptTokens += uint64(u.PromptTokens)
	c.tot.CompletionTokens += uint64(u.CompletionTokens)
}

type chatRequest struct {
	Model          string            `json:"model"`
	Messages       []Message         `json:"messages"`
	MaxTokens      int               `json:"max_tokens"`
	Temperature    float64           `json:"temperature"`
	ResponseFormat map[string]string `json:"response_format,omitempty"`
}

type chatResponse struct {
	ID      string `json:"id"`
	Model   string `json:"model"`
	Choices []struct {
		FinishReason string `json:"finish_reason"`
		Message      struct {
			Content json.RawMessage `json:"content"`
			Refusal *string         `json:"refusal"`
		} `json:"message"`
	} `json:"choices"`
	Usage *struct {
		PromptTokens     int `json:"prompt_tokens"`
		CompletionTokens int `json:"completion_tokens"`
	} `json:"usage"`
	Error *struct {
		Message string `json:"message"`
		Code    any    `json:"code"`
	} `json:"error"`
}

// Complete implements Completer.
func (c *Client) Complete(ctx context.Context, req Request) (resp *Response, err error) {
	started := c.clk.Now()
	outcome := "error"
	var usage Usage
	defer func() {
		if err == nil {
			outcome = "ok"
		} else {
			c.mu.Lock()
			c.tot.Failures++
			c.mu.Unlock()
		}
		if c.obs != nil {
			c.obs.ObserveLLM(req.Purpose, outcome, usage.PromptTokens, usage.CompletionTokens, c.clk.Now().Sub(started))
		}
	}()
	if len(req.Messages) == 0 {
		return nil, errors.New("llm: no messages")
	}
	size := 0
	for _, m := range req.Messages {
		size += len(m.Content)
	}
	if size > c.cfg.MaxInputBytes {
		outcome = "invalid"
		return nil, fmt.Errorf("%w: %d bytes, limit %d", ErrInputTooLarge, size, c.cfg.MaxInputBytes)
	}
	body := chatRequest{Model: c.cfg.Model, Messages: req.Messages, MaxTokens: c.cfg.MaxOutputTokens, Temperature: c.cfg.Temperature}
	if req.MaxOutputTokens > 0 {
		body.MaxTokens = req.MaxOutputTokens
	}
	if c.cfg.JSONMode {
		body.ResponseFormat = map[string]string{"type": "json_object"}
	}
	payload, err := json.Marshal(body)
	if err != nil {
		return nil, err
	}

	select {
	case c.sem <- struct{}{}:
		defer func() { <-c.sem }()
	case <-ctx.Done():
		return nil, ctx.Err()
	}

	var lastErr error
	for attempt := 1; attempt <= c.cfg.Retries+1; attempt++ {
		if berr := c.reserve(); berr != nil {
			outcome = "budget"
			return nil, berr
		}
		r, aerr := c.once(ctx, payload)
		if aerr == nil {
			r.Attempts, r.Duration = attempt, c.clk.Now().Sub(started)
			usage = r.Usage
			c.record(r.Usage)
			if r.FinishReason == "length" {
				outcome = "truncated"
				return r, fmt.Errorf("%w (finish_reason=length, %d completion tokens)", ErrTruncated, r.Usage.CompletionTokens)
			}
			if strings.TrimSpace(r.Text) == "" {
				return r, ErrEmpty
			}
			return r, nil
		}
		lastErr = aerr
		var le *Error
		if ctx.Err() != nil || !errors.As(aerr, &le) || !le.Retryable || attempt > c.cfg.Retries {
			break
		}
		wait := c.backoff(attempt)
		if le.RetryAfter > 0 {
			wait = max(wait, min(le.RetryAfter, 30*time.Second))
		}
		c.log.Debug("llm request failed; retrying", "purpose", req.Purpose, "attempt", attempt, "status", le.Status, "wait", wait)
		if serr := clock.Sleep(ctx, c.clk, wait); serr != nil {
			return nil, serr
		}
	}
	return nil, lastErr
}

func (c *Client) backoff(attempt int) time.Duration {
	d := float64(c.cfg.RetryBase) * math.Pow(2, float64(attempt-1))
	d = math.Min(d, float64(c.cfg.RetryMax))
	return time.Duration(d * (0.8 + 0.4*c.jit()))
}

// once makes one HTTP attempt.
func (c *Client) once(ctx context.Context, payload []byte) (*Response, error) {
	actx, cancel := context.WithTimeout(ctx, c.cfg.Timeout)
	defer cancel()
	req, err := http.NewRequestWithContext(actx, http.MethodPost, c.url, bytes.NewReader(payload))
	if err != nil {
		return nil, &Error{Message: "cannot build the request: " + c.scrub(err.Error())}
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", "agentgit-orchestrator")
	for k, v := range c.cfg.Headers {
		req.Header.Set(k, v)
	}
	if !c.cfg.APIKey.Empty() {
		req.Header.Set("Authorization", "Bearer "+c.cfg.APIKey.Reveal())
	}
	res, err := c.hc.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		// transport failures (reset, timeout of this attempt, DNS) are worth repeating
		return nil, &Error{Message: "request failed: " + c.scrub(urlErrorText(err)), Retryable: true}
	}
	defer res.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(res.Body, 4<<20))
	if err != nil {
		return nil, &Error{Status: res.StatusCode, Message: "reading the reply failed: " + c.scrub(err.Error()), Retryable: true}
	}
	var cr chatResponse
	jerr := json.Unmarshal(raw, &cr)
	if res.StatusCode/100 != 2 {
		e := &Error{Status: res.StatusCode, Retryable: retryableStatus(res.StatusCode)}
		if jerr == nil && cr.Error != nil {
			e.Message = c.scrub(truncate(cr.Error.Message, 300))
			if cr.Error.Code != nil {
				e.Code = fmt.Sprint(cr.Error.Code)
			}
		} else {
			e.Message = c.scrub(truncate(strings.TrimSpace(string(raw)), 200))
		}
		if ra := res.Header.Get("Retry-After"); ra != "" {
			if secs, perr := strconv.Atoi(strings.TrimSpace(ra)); perr == nil && secs >= 0 {
				e.RetryAfter = time.Duration(secs) * time.Second
			}
		}
		return nil, e
	}
	if jerr != nil {
		return nil, &Error{Status: res.StatusCode, Message: "the reply is not JSON: " + c.scrub(jerr.Error()), Retryable: false}
	}
	if cr.Error != nil { // some gateways answer 200 with an error object
		return nil, &Error{Status: res.StatusCode, Message: c.scrub(truncate(cr.Error.Message, 300)), Retryable: false}
	}
	if len(cr.Choices) == 0 {
		return nil, &Error{Status: res.StatusCode, Message: "the reply has no choices"}
	}
	ch := cr.Choices[0]
	if ch.Message.Refusal != nil && *ch.Message.Refusal != "" {
		return nil, &Error{Status: res.StatusCode, Message: "the model refused: " + c.scrub(truncate(*ch.Message.Refusal, 200))}
	}
	text := contentText(ch.Message.Content)
	out := &Response{Text: text, Model: cr.Model, FinishReason: ch.FinishReason, RequestID: cr.ID}
	if cr.Usage != nil {
		out.Usage = Usage{PromptTokens: cr.Usage.PromptTokens, CompletionTokens: cr.Usage.CompletionTokens}
	} else {
		// no usage reported: estimate (about four bytes per token) so the budget still moves
		out.Usage = Usage{PromptTokens: len(payload) / 4, CompletionTokens: len(text) / 4, Estimated: true}
	}
	return out, nil
}

// contentText reads message.content, which is a string, or (in newer APIs) a list of typed parts.
func contentText(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s
	}
	var parts []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if err := json.Unmarshal(raw, &parts); err == nil {
		var b strings.Builder
		for _, p := range parts {
			if p.Type == "" || p.Type == "text" {
				b.WriteString(p.Text)
			}
		}
		return b.String()
	}
	return ""
}

func retryableStatus(s int) bool {
	switch s {
	case http.StatusRequestTimeout, http.StatusTooManyRequests, http.StatusBadGateway, http.StatusServiceUnavailable, http.StatusGatewayTimeout, http.StatusInternalServerError:
		return true
	}
	return false
}

// urlErrorText drops the URL from a transport error (it can carry credentials in a userinfo part).
func urlErrorText(err error) string {
	var ue *url.Error
	if errors.As(err, &ue) {
		return ue.Err.Error()
	}
	return err.Error()
}

func (c *Client) scrub(s string) string { return secret.Scrub(s, c.cfg.APIKey) }

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "..."
}
