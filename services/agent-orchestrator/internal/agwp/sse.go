package agwp

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
)

// FollowOptions configures a Follower.
type FollowOptions struct {
	// Repo is "owner/name".
	Repo string
	// Topics filters by dotted prefix ("task", "stream.integrate"); empty = all.
	Topics []string
	// AfterSeq is where to resume. Zero together with FromStart=false means
	// "from now": the follower first reads the log's newest sequence number.
	AfterSeq uint64
	// FromStart replays the whole log first.
	FromStart bool
	// IdleTimeout reconnects when the server has been silent this long
	// (heartbeats arrive every HeartbeatMs). Default 60s.
	IdleTimeout time.Duration
	// HeartbeatMs asks the server for SSE heartbeats at this interval (min 1000; default 15000).
	HeartbeatMs int
	// BackoffBase and BackoffMax shape the reconnect delay (defaults 500ms and 30s).
	BackoffBase, BackoffMax time.Duration
	// OnReconnect is called before every reconnection attempt.
	OnReconnect func(err error)
	// Logger receives connection-level logs.
	Logger *slog.Logger
}

// Follower reads a repository's event stream forever, reconnecting with
// exponential backoff and resuming from the last sequence number it handled,
// so a restart of either side loses nothing the log still holds.
type Follower struct {
	c       *Client
	opt     FollowOptions
	lastSeq atomic.Uint64
	log     *slog.Logger
}

// NewFollower prepares a follower; call Run to start it.
func (c *Client) NewFollower(opt FollowOptions) *Follower {
	if opt.IdleTimeout <= 0 {
		opt.IdleTimeout = 60 * time.Second
	}
	if opt.HeartbeatMs < 1000 {
		opt.HeartbeatMs = 15000
	}
	if opt.BackoffBase <= 0 {
		opt.BackoffBase = 500 * time.Millisecond
	}
	if opt.BackoffMax <= 0 {
		opt.BackoffMax = 30 * time.Second
	}
	f := &Follower{c: c, opt: opt, log: opt.Logger}
	if f.log == nil {
		f.log = slog.Default()
	}
	f.log = f.log.With("repo", opt.Repo, "component", "sse")
	f.lastSeq.Store(opt.AfterSeq)
	return f
}

// LastSeq is the sequence number of the last event handed to the handler.
func (f *Follower) LastSeq() uint64 { return f.lastSeq.Load() }

// Run delivers events to handle, in order, until ctx is cancelled. Events are
// handled synchronously: a slow handler applies backpressure to the stream.
func (f *Follower) Run(ctx context.Context, handle func(Event)) error {
	started := false
	attempt := 0
	for {
		if !started && !f.opt.FromStart && f.lastSeq.Load() == 0 {
			_, latest, err := f.c.ReplayEvents(ctx, f.opt.Repo, 0, 1, f.opt.Topics)
			if err == nil {
				f.lastSeq.Store(latest)
				started = true
			} else if ctx.Err() != nil {
				return ctx.Err()
			} else {
				f.log.Warn("cannot read the event log position", "err", err)
				if serr := f.wait(ctx, &attempt); serr != nil {
					return serr
				}
				continue
			}
		}
		started = true
		before := f.lastSeq.Load()
		began := f.c.clk.Now()
		err := f.stream(ctx, handle)
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if f.lastSeq.Load() != before || f.c.clk.Now().Sub(began) > 10*time.Second {
			attempt = 0 // the connection was healthy; start the backoff over
		}
		if f.opt.OnReconnect != nil {
			f.opt.OnReconnect(err)
		}
		f.log.Warn("event stream ended; reconnecting", "err", err, "after_seq", f.lastSeq.Load())
		if serr := f.wait(ctx, &attempt); serr != nil {
			return serr
		}
	}
}

func (f *Follower) wait(ctx context.Context, attempt *int) error {
	d := f.opt.BackoffBase << min(*attempt, 20)
	if d <= 0 || d > f.opt.BackoffMax {
		d = f.opt.BackoffMax
	}
	d = time.Duration(float64(d) * (0.8 + 0.4*f.c.jitter()))
	*attempt++
	return clock.Sleep(ctx, f.c.clk, d)
}

// stream runs one connection until it ends or fails.
func (f *Follower) stream(ctx context.Context, handle func(Event)) error {
	p, err := repoPath(f.opt.Repo)
	if err != nil {
		return err
	}
	q := url.Values{
		"after_seq":    {strconv.FormatUint(f.lastSeq.Load(), 10)},
		"heartbeat_ms": {strconv.Itoa(f.opt.HeartbeatMs)},
	}
	if len(f.opt.Topics) > 0 {
		q.Set("topics", strings.Join(f.opt.Topics, ","))
	}
	u, err := f.c.urlFor(p+"/events", q)
	if err != nil {
		return err
	}
	sctx, cancel := context.WithCancel(ctx)
	defer cancel()
	req, err := http.NewRequestWithContext(sctx, http.MethodGet, u, nil)
	if err != nil {
		return err
	}
	f.c.decorate(req, "", 0)
	req.Header.Del("Request-Timeout")
	req.Header.Set("Accept", "text/event-stream")
	req.Header.Set("Cache-Control", "no-cache")
	if last := f.lastSeq.Load(); last > 0 {
		req.Header.Set("Last-Event-ID", strconv.FormatUint(last, 10))
	}
	f.c.obs.ObserveRequest(f.c.identity, "events.subscribe")
	resp, err := f.c.stream.Do(req)
	if err != nil {
		return scrubURL(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		data, _ := io.ReadAll(io.LimitReader(resp.Body, 64<<10))
		e := parseError(resp, data)
		if ae, ok := e.(*Error); ok {
			f.c.obs.ObserveError(ae.Code)
		}
		return e
	}
	idle := time.AfterFunc(f.opt.IdleTimeout, cancel)
	defer idle.Stop()
	return readSSE(resp.Body, func() { idle.Reset(f.opt.IdleTimeout) }, func(ev sseEvent) error {
		if ev.name == "error" {
			var env struct {
				Error struct {
					Code    string `json:"code"`
					Message string `json:"message"`
				} `json:"error"`
			}
			_ = json.Unmarshal([]byte(ev.data), &env)
			f.c.obs.ObserveError(env.Error.Code)
			return &Error{Code: env.Error.Code, Message: env.Error.Message}
		}
		var e Event
		if err := json.Unmarshal([]byte(ev.data), &e); err != nil {
			f.log.Warn("skipping an unparseable event", "id", ev.id, "err", err)
			return nil
		}
		if e.Seq == 0 {
			return nil
		}
		if e.Seq <= f.lastSeq.Load() {
			return nil // already handled (a replay overlapped a reconnect)
		}
		handle(e)
		f.lastSeq.Store(e.Seq)
		return nil
	})
}

type sseEvent struct {
	id, name, data string
}

const maxSSELine = 1 << 20

var errStreamClosed = errors.New("event stream closed by the server")

// readSSE parses text/event-stream from r, calling touch on every line read
// and emit for every complete event. It returns when the stream ends or emit
// fails.
func readSSE(r io.Reader, touch func(), emit func(sseEvent) error) error {
	br := bufio.NewReaderSize(r, 64<<10)
	var ev sseEvent
	var data []string
	for {
		line, err := readLine(br)
		if err != nil {
			if errors.Is(err, io.EOF) {
				return errStreamClosed
			}
			return err
		}
		touch()
		switch {
		case line == "":
			if len(data) > 0 || ev.name != "" {
				ev.data = strings.Join(data, "\n")
				if err := emit(ev); err != nil {
					return err
				}
			}
			ev, data = sseEvent{}, nil
		case strings.HasPrefix(line, ":"):
			// comment / heartbeat
		default:
			field, value, _ := strings.Cut(line, ":")
			value = strings.TrimPrefix(value, " ")
			switch field {
			case "id":
				ev.id = value
			case "event":
				ev.name = value
			case "data":
				data = append(data, value)
			}
		}
	}
}

// readLine reads one line without its terminator, refusing absurdly long ones.
func readLine(br *bufio.Reader) (string, error) {
	var sb strings.Builder
	for {
		chunk, isPrefix, err := br.ReadLine()
		if err != nil {
			return "", err
		}
		sb.Write(chunk)
		if sb.Len() > maxSSELine {
			return "", fmt.Errorf("agwp: SSE line longer than %d bytes", maxSSELine)
		}
		if !isPrefix {
			return sb.String(), nil
		}
	}
}
