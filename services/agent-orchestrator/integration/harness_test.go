//go:build linux

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/app"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

const repoName = testutil.RepoName

type worldOptions = testutil.WorldOptions

// world is the shared real-server world (internal/testutil) plus the helpers
// only these scenarios use.
type world struct{ *testutil.World }

func newWorld(t *testing.T, o worldOptions) *world { return &world{testutil.NewWorld(t, o)} }

func randHex(n int) string { return testutil.RandHex(n) }

func put(path, content string) agwp.FileChange { return testutil.Put(path, content) }

// script is a task description the test agent (internal/testutil/testagent,
// TESTAGENT_TASK_FILES=1) understands.
func script(title string, files map[string]string) string {
	b, _ := json.Marshal(map[string]any{"files": files, "title": title, "message": title})
	return "Test agent change.\nag-script:" + string(b)
}

// ---- the orchestrator under test -------------------------------------------------------

type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

type orchConfig struct {
	// Repo is extra YAML under the repository entry, indented by four spaces.
	Repo string
	// Executors is the YAML list of executors, each item indented by two spaces.
	Executors string
	// Extra is extra top-level YAML.
	Extra string
	// Shutdown is the body of the shutdown mapping, indented two spaces (default: grace 20s).
	Shutdown string
	// StateDir reuses a state directory (keys, index) across orchestrator restarts.
	StateDir string
	// Admin makes the admin API require this token (via AG_ORCH_ADMIN).
	AdminToken string
	// Clock replaces the wall clock (a skewed clock lets a test make a run look stale).
	Clock clock.Clock
}

// skewClock is the wall clock plus an adjustable offset: timers and tickers are
// real, only Now moves. Tests use it to make server-side timestamps look old to
// the orchestrator without waiting for them to age.
type skewClock struct {
	clock.Real
	offset atomic.Int64
}

func (c *skewClock) Now() time.Time { return time.Now().Add(time.Duration(c.offset.Load())) }

func (c *skewClock) Skew(d time.Duration) { c.offset.Add(int64(d)) }

type orchestrator struct {
	t      testing.TB
	w      *world
	app    *app.App
	cfg    *config.Config
	base   string
	logs   *syncBuffer
	tokens map[string]string
	state  string

	cancel  context.CancelFunc
	done    chan error
	stopped sync.Once
	stopErr error
}

func (w *world) startOrchestrator(oc orchConfig) *orchestrator {
	t := w.T
	t.Helper()
	state := oc.StateDir
	if state == "" {
		state = t.TempDir()
	}
	shutdown := oc.Shutdown
	if shutdown == "" {
		shutdown = "  grace: 20s"
	}
	doc := fmt.Sprintf(`
server:
  url: %s
  timeout: 20s
  retries: 3
state_dir: %s
listen: 127.0.0.1:0
log:
  level: debug
loop:
  reconcile: 1s
  debounce: 20ms
shutdown:
%s
%s
repos:
  - name: %s
%s
executors:
%s
`, w.Srv.URL, state, shutdown, oc.Extra, w.Repo, oc.Repo, oc.Executors)
	env := map[string]string{"AG_ORCH_TOKEN": "placeholder"}
	if oc.AdminToken != "" {
		doc = strings.Replace(doc, "listen: 127.0.0.1:0\n", "listen: 127.0.0.1:0\nadmin:\n  token_env: AG_ORCH_ADMIN\n", 1)
		env["AG_ORCH_ADMIN"] = oc.AdminToken
	}
	getenv := func(k string) string { return env[k] }
	cfg, err := config.Parse(strings.NewReader(doc), getenv)
	if err != nil {
		t.Fatalf("orchestrator configuration: %v\n%s", err, doc)
	}
	env["AG_ORCH_TOKEN"] = w.Mint("service:orchestrator", app.OrchestratorOps(cfg))
	env["AG_REVIEWER_TOKEN"] = w.Mint("service:reviewer", app.ReviewerOps())
	env["AG_ADVISOR_TOKEN"] = w.Mint("service:advisor", app.AdvisorOps())
	env["PATH"] = os.Getenv("PATH")

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	logs := &syncBuffer{}
	a, err := app.New(app.Options{Config: cfg, Getenv: getenv, Clock: oc.Clock, Log: slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelDebug})), Listener: ln})
	if err != nil {
		t.Fatalf("app.New: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	o := &orchestrator{t: t, w: w, app: a, cfg: cfg, base: "http://" + ln.Addr().String(), logs: logs, tokens: env, state: state, cancel: cancel, done: make(chan error, 1)}
	go func() { o.done <- a.Run(ctx) }()
	t.Cleanup(func() {
		o.stop()
		if t.Failed() {
			s := logs.String()
			if len(s) > 12000 {
				s = "...\n" + s[len(s)-12000:]
			}
			t.Logf("orchestrator log (tail):\n%s", s)
		}
	})
	return o
}

// stop shuts the orchestrator down (SIGTERM semantics) and returns when Run has.
func (o *orchestrator) stop() error {
	o.stopped.Do(func() {
		o.cancel()
		select {
		case o.stopErr = <-o.done:
		case <-time.After(90 * time.Second):
			o.t.Error("the orchestrator did not stop within 90s")
		}
	})
	return o.stopErr
}

func (o *orchestrator) get(path string) (int, string) {
	o.t.Helper()
	req, _ := http.NewRequest("GET", o.base+path, nil)
	if tok := o.tokens["AG_ORCH_ADMIN"]; tok != "" {
		req.Header.Set("Authorization", "Bearer "+tok)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		o.t.Fatalf("GET %s: %v", path, err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, string(b)
}

func (o *orchestrator) status() app.Status {
	o.t.Helper()
	code, body := o.get("/status")
	if code != 200 {
		o.t.Fatalf("/status = %d %s", code, body)
	}
	var st app.Status
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		o.t.Fatalf("/status: %v\n%s", err, body)
	}
	return st
}

// waitReady waits until the orchestrator reports readiness (connected, authenticated, streams up).
func (o *orchestrator) waitReady() {
	o.t.Helper()
	eventually(o.t, "the orchestrator to become ready", 30*time.Second, func() bool {
		code, _ := o.get("/readyz")
		return code == 200
	})
}

// secrets are every credential the orchestrator was given; none may appear in its output.
func (o *orchestrator) secrets() []string {
	var out []string
	for k, v := range o.tokens {
		if strings.HasPrefix(k, "AG_") && len(v) > 12 {
			out = append(out, v)
		}
	}
	return out
}

func eventually(t testing.TB, what string, within time.Duration, f func() bool) {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if f() {
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("timed out after %s waiting for %s", within, what)
}

// writeExecutable writes a shell script and returns its path.
func writeScript(t testing.TB, name, body string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, []byte("#!/bin/sh\n"+body), 0o700); err != nil {
		t.Fatal(err)
	}
	return p
}
