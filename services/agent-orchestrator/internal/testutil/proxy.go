package testutil

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// Request is one request that passed through (or was stopped by) a Proxy.
type Request struct {
	Method, Path, Query string
	Header              http.Header // the Authorization header is not recorded
	Body                string      // at most 1 MiB
	Faulted             bool        // answered by an injected fault, never reached the server
}

// Fault describes what a Proxy does to requests that match.
type Fault struct {
	// Method and Path select requests: Method "" matches any, Path is a substring of the URL path ("" matches any).
	Method, Path string
	// Times is how many matching requests are hit (<0: every one).
	Times int
	// Drop closes the connection without a response; otherwise an AGWP error with Status is answered.
	Drop         bool
	Status       int
	Code         string
	Retryable    bool
	RetryAfterMs int64
}

// Proxy is a forwarding HTTP proxy in front of a real ag-server. It records
// the traffic and injects the faults a healthy server cannot be made to
// produce on demand (see the package comment). Everything else reaches the
// real server unchanged, including long-lived SSE streams.
type Proxy struct {
	URL string

	t        testing.TB
	srv      *httptest.Server
	mu       sync.Mutex
	log      []Request
	faults   []*Fault
	rewrites []rewrite
	conns    map[int]context.CancelFunc // open SSE streams
	nextID   int
}

// NewProxy starts a proxy to target (the base URL of a real server).
func NewProxy(t testing.TB, target string) *Proxy {
	t.Helper()
	u, err := url.Parse(target)
	if err != nil {
		t.Fatal(err)
	}
	p := &Proxy{t: t, conns: map[int]context.CancelFunc{}}
	rp := httputil.NewSingleHostReverseProxy(u)
	rp.FlushInterval = -1                    // SSE
	rp.ErrorLog = log.New(io.Discard, "", 0) // a cut stream is not an error worth printing
	rp.ModifyResponse = p.rewrite
	p.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { p.serve(rp, w, r) }))
	p.URL = p.srv.URL
	t.Cleanup(p.Close)
	return p
}

// Close stops the proxy and cuts every open stream.
func (p *Proxy) Close() {
	p.DropEventStreams()
	p.srv.CloseClientConnections()
	p.srv.Close()
}

func (p *Proxy) serve(rp *httputil.ReverseProxy, w http.ResponseWriter, r *http.Request) {
	var body []byte
	if r.Body != nil {
		body, _ = io.ReadAll(io.LimitReader(r.Body, 1<<20))
		rest, _ := io.ReadAll(r.Body)
		r.Body = io.NopCloser(io.MultiReader(bytes.NewReader(body), bytes.NewReader(rest)))
	}
	rec := Request{Method: r.Method, Path: r.URL.Path, Query: r.URL.RawQuery, Header: r.Header.Clone(), Body: string(body)}
	rec.Header.Del("Authorization")

	p.mu.Lock()
	var hit *Fault
	for _, f := range p.faults {
		if f.Times == 0 || (f.Method != "" && f.Method != r.Method) || !strings.Contains(r.URL.Path, f.Path) {
			continue
		}
		hit = f
		if f.Times > 0 {
			f.Times--
		}
		break
	}
	rec.Faulted = hit != nil
	p.log = append(p.log, rec)
	p.mu.Unlock()

	if hit != nil {
		if hit.Drop {
			if hj, ok := w.(http.Hijacker); ok {
				if c, _, err := hj.Hijack(); err == nil {
					c.Close()
					return
				}
			}
			panic(http.ErrAbortHandler)
		}
		w.Header().Set("Content-Type", "application/json")
		if hit.RetryAfterMs > 0 {
			w.Header().Set("Retry-After", strconv.FormatInt((hit.RetryAfterMs+999)/1000, 10))
		}
		w.WriteHeader(hit.Status)
		_ = json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{
			"code": hit.Code, "message": "injected failure", "retryable": hit.Retryable, "retry_after_ms": hit.RetryAfterMs,
			"request_id": "req_injected", "details": map[string]string{},
		}})
		return
	}
	if strings.HasSuffix(r.URL.Path, "/events") && strings.Contains(r.Header.Get("Accept"), "text/event-stream") {
		ctx, cancel := context.WithCancel(r.Context())
		p.mu.Lock()
		id := p.nextID
		p.nextID++
		p.conns[id] = cancel
		p.mu.Unlock()
		defer func() {
			p.mu.Lock()
			delete(p.conns, id)
			p.mu.Unlock()
			cancel()
		}()
		r = r.WithContext(ctx)
	}
	rp.ServeHTTP(w, r)
}

// RewriteJSON makes the proxy edit the JSON object the server answers for GET requests whose path ends in
// pathSuffix (what an older server version would have answered). fn changes the decoded object in place.
func (p *Proxy) RewriteJSON(pathSuffix string, fn func(obj map[string]any)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.rewrites = append(p.rewrites, rewrite{suffix: pathSuffix, fn: fn})
}

type rewrite struct {
	suffix string
	fn     func(map[string]any)
}

func (p *Proxy) rewrite(resp *http.Response) error {
	p.mu.Lock()
	var fns []func(map[string]any)
	for _, r := range p.rewrites {
		if resp.Request.Method == http.MethodGet && strings.HasSuffix(resp.Request.URL.Path, r.suffix) {
			fns = append(fns, r.fn)
		}
	}
	p.mu.Unlock()
	if len(fns) == 0 || resp.StatusCode != http.StatusOK {
		return nil
	}
	b, err := io.ReadAll(resp.Body)
	resp.Body.Close()
	if err != nil {
		return err
	}
	var obj map[string]any
	if err := json.Unmarshal(b, &obj); err != nil {
		return err
	}
	for _, fn := range fns {
		fn(obj)
	}
	out, err := json.Marshal(obj)
	if err != nil {
		return err
	}
	resp.Body = io.NopCloser(bytes.NewReader(out))
	resp.ContentLength = int64(len(out))
	resp.Header.Set("Content-Length", strconv.Itoa(len(out)))
	resp.Header.Del("Content-Encoding")
	return nil
}

// Inject adds a fault.
func (p *Proxy) Inject(f Fault) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.faults = append(p.faults, &f)
}

// ClearFaults removes every fault.
func (p *Proxy) ClearFaults() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.faults = nil
}

// DropEventStreams cuts every open SSE stream (the server stays up).
func (p *Proxy) DropEventStreams() {
	p.mu.Lock()
	cancels := make([]context.CancelFunc, 0, len(p.conns))
	for _, c := range p.conns {
		cancels = append(cancels, c)
	}
	p.mu.Unlock()
	for _, c := range cancels {
		c()
	}
}

// OpenEventStreams is how many SSE streams are open now.
func (p *Proxy) OpenEventStreams() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.conns)
}

// Requests is a copy of everything seen so far.
func (p *Proxy) Requests() []Request {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]Request(nil), p.log...)
}

// Count is how many requests had this method ("" any) and a path containing pathPart, faulted or not.
func (p *Proxy) Count(method, pathPart string) int {
	p.mu.Lock()
	defer p.mu.Unlock()
	n := 0
	for _, r := range p.log {
		if (method == "" || r.Method == method) && strings.Contains(r.Path, pathPart) {
			n++
		}
	}
	return n
}
