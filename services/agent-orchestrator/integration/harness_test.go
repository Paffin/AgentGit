//go:build linux

package integration

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/app"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

const repoName = "acme/web"

func randHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b)
}

func findServerBin() string {
	if p := os.Getenv("AG_SERVER_BIN"); p != "" {
		return p
	}
	if p, err := exec.LookPath("ag-server"); err == nil {
		return p
	}
	if _, err := os.Stat("/home/user/bin/ag-server"); err == nil {
		return "/home/user/bin/ag-server"
	}
	return ""
}

func freePort(t *testing.T) int {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

// ---- the real server ------------------------------------------------------------------

type server struct {
	t       *testing.T
	URL     string
	token   string
	logPath string
	cmd     *exec.Cmd
	done    chan struct{}
	once    sync.Once
	admin   *agwp.Client
	hc      *http.Client
}

// startServer runs a fresh ag-server on a free loopback port. It is killed
// when the test ends whatever the outcome, and also if the test binary dies.
func startServer(t *testing.T) *server {
	t.Helper()
	bin := findServerBin()
	if bin == "" {
		t.Skip("integration tests need the ag-server binary: set AG_SERVER_BIN, put ag-server on PATH, or install /home/user/bin/ag-server")
	}
	dir := t.TempDir()
	token := randHex(24) // 48 characters; the server wants at least 32
	for attempt := 0; attempt < 5; attempt++ {
		addr := fmt.Sprintf("127.0.0.1:%d", freePort(t))
		logPath := filepath.Join(dir, fmt.Sprintf("server-%d.log", attempt))
		lf, err := os.Create(logPath)
		if err != nil {
			t.Fatal(err)
		}
		cmd := exec.Command(bin, "serve", "--data-dir", filepath.Join(dir, fmt.Sprintf("data-%d", attempt)), "--listen", addr)
		cmd.Env = []string{"AG_BOOTSTRAP_TOKEN=" + token, "PATH=" + os.Getenv("PATH"), "HOME=" + dir}
		cmd.Stdout, cmd.Stderr = lf, lf
		cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true, Pdeathsig: syscall.SIGKILL}
		if err := cmd.Start(); err != nil {
			t.Fatalf("cannot start %s: %v", bin, err)
		}
		lf.Close()
		s := &server{t: t, URL: "http://" + addr, token: token, logPath: logPath, cmd: cmd, done: make(chan struct{}), hc: &http.Client{Timeout: 30 * time.Second}}
		go func() { _ = cmd.Wait(); close(s.done) }()
		t.Cleanup(s.stop)
		var err2 error
		s.admin, err2 = agwp.New(s.URL, secret.New(token), agwp.WithIdentityLabel("test-admin"), agwp.WithTimeout(30*time.Second))
		if err2 != nil {
			t.Fatal(err2)
		}
		if s.waitReady(15 * time.Second) {
			return s
		}
		s.stop()
	}
	t.Fatal("the server did not start in five attempts")
	return nil
}

func (s *server) waitReady(d time.Duration) bool {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		select {
		case <-s.done:
			return false
		default:
		}
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		_, err := s.admin.Negotiate(ctx)
		cancel()
		if err == nil {
			return true
		}
		time.Sleep(50 * time.Millisecond)
	}
	return false
}

func (s *server) stop() {
	s.once.Do(func() {
		if s.cmd.Process != nil {
			select {
			case <-s.done:
			default:
				_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGTERM)
				select {
				case <-s.done:
				case <-time.After(3 * time.Second):
					_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGKILL)
					<-s.done
				}
			}
		}
		if s.t.Failed() {
			if b, err := os.ReadFile(s.logPath); err == nil {
				if len(b) > 6000 {
					b = b[len(b)-6000:]
				}
				s.t.Logf("ag-server log (tail):\n%s", b)
			}
		}
	})
}

// call is a raw HTTP call for what the typed client does not cover (the
// execution-plane worker routes, negative tests with borrowed credentials).
func (s *server) call(token, method, path string, body any, headers ...string) (int, []byte) {
	s.t.Helper()
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			s.t.Fatal(err)
		}
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, s.URL+path, rd)
	if err != nil {
		s.t.Fatal(err)
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for i := 0; i+1 < len(headers); i += 2 {
		req.Header.Set(headers[i], headers[i+1])
	}
	resp, err := s.hc.Do(req)
	if err != nil {
		s.t.Fatalf("%s %s: %v", method, path, err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, b
}

func (s *server) client(token string, label string) *agwp.Client {
	s.t.Helper()
	c, err := agwp.New(s.URL, secret.New(token), agwp.WithIdentityLabel(label), agwp.WithTimeout(30*time.Second))
	if err != nil {
		s.t.Fatal(err)
	}
	return c
}

// ---- the world: a repository, people, a CI worker -------------------------------------

type world struct {
	t     *testing.T
	srv   *server
	repo  string
	alice *agwp.Client // writer: files tasks, prepares workspaces
	bob   *agwp.Client // reviewer: approves
	carol *agwp.Client // integrator: the only one who lands changes
	ci    *ciWorker

	aliceTok, bobTok, carolTok string
}

// updateProposal moves a proposal to a new head (what a person does after
// resolving conflicts in a workspace).
func (w *world) updateProposal(id, head string) {
	w.t.Helper()
	code, body := w.srv.call(w.aliceTok, "PATCH", "/v1/repos/"+w.repo+"/proposals/"+id, map[string]any{"new_head": head})
	if code != 200 {
		w.t.Fatalf("update proposal %s: %d %s", id, code, body)
	}
}

type worldOptions struct {
	// Policy returns the policy document for the main stream, given the public
	// key (hex) of the CI worker the policy should trust. Nil: no policy.
	Policy func(ciPublicKey string) string
	// Agents are the agent names that get the writer role (and so can be given
	// credentials by the orchestrator).
	Agents []string
}

func newWorld(t *testing.T, o worldOptions) *world {
	t.Helper()
	srv := startServer(t)
	ctx := context.Background()
	w := &world{t: t, srv: srv, repo: repoName}
	if _, err := srv.admin.CreateRepo(ctx, repoName); err != nil {
		t.Fatalf("create repo: %v", err)
	}
	grant := func(subject, role string) {
		t.Helper()
		if err := srv.admin.GrantRole(ctx, subject, repoName, role); err != nil {
			t.Fatalf("grant %s %s: %v", subject, role, err)
		}
	}
	for subject, role := range map[string]string{
		"user:alice": "writer", "user:bob": "reviewer", "user:carol": "integrator",
		"service:orchestrator": "admin", "service:reviewer": "reviewer", "service:advisor": "reviewer", "service:ci-1": "runner",
	} {
		grant(subject, role)
	}
	for _, a := range o.Agents {
		grant("agent:"+a, "writer")
	}
	w.aliceTok, w.bobTok, w.carolTok = w.mint("user:alice", nil), w.mint("user:bob", nil), w.mint("user:carol", nil)
	w.alice, w.bob, w.carol = srv.client(w.aliceTok, "alice"), srv.client(w.bobTok, "bob"), srv.client(w.carolTok, "carol")

	key, err := agentkey.Generate()
	if err != nil {
		t.Fatal(err)
	}
	w.ci = &ciWorker{w: w, issuer: "service:ci-1", key: key, token: w.mint("service:ci-1", nil)}
	if o.Policy != nil {
		if err := srv.admin.SetPolicy(ctx, repoName, "main", o.Policy(key.PublicHex())); err != nil {
			t.Fatalf("set policy: %v", err)
		}
	}
	return w
}

// mint asks the server for a credential of subject (as an administrator).
func (w *world) mint(subject string, ops []string) string {
	w.t.Helper()
	m, err := w.srv.admin.MintCredential(context.Background(), agwp.MintRequest{
		Subject: subject, TTLSecs: 7 * 24 * 3600, Description: "integration test", Scope: agwp.CredentialScope{Operations: ops},
	})
	if err != nil {
		w.t.Fatalf("mint credential for %s: %v", subject, err)
	}
	return m.Token.Reveal()
}

func put(path, content string) agwp.FileChange {
	return agwp.FileChange{Path: path, Op: "put", Content: []byte(content)}
}

func del(path string) agwp.FileChange { return agwp.FileChange{Path: path, Op: "delete"} }

// propose opens a proposal against main from a fresh workspace, as alice.
func (w *world) propose(ws, title string, changes ...agwp.FileChange) (*agwp.Proposal, *agwp.Workspace) {
	w.t.Helper()
	ctx := context.Background()
	if _, err := w.alice.CreateWorkspace(ctx, w.repo, ws, "main"); err != nil {
		w.t.Fatalf("create workspace %s: %v", ws, err)
	}
	wsp, err := w.alice.ModifyWorkspace(ctx, w.repo, ws, title, "", changes)
	if err != nil {
		w.t.Fatalf("modify workspace %s: %v", ws, err)
	}
	p, err := w.alice.CreateProposal(ctx, w.repo, agwp.ProposalInput{Stream: "main", Head: wsp.Head, Title: title})
	if err != nil {
		w.t.Fatalf("open proposal %q: %v", title, err)
	}
	return p, wsp
}

// land integrates a proposal as carol and requires that it worked.
func (w *world) land(p *agwp.Proposal) {
	w.t.Helper()
	res, err := w.carol.Integrate(context.Background(), w.repo, p.ID, nil)
	if err != nil {
		w.t.Fatalf("integrate %s: %v", p.ID, err)
	}
	if res.Outcome.Status != "integrated" {
		w.t.Fatalf("proposal %s did not integrate: %+v", p.ID, res.Outcome)
	}
}

// seed lands a first change made by a human.
func (w *world) seed(files map[string]string) {
	w.t.Helper()
	var ch []agwp.FileChange
	for p, c := range files {
		ch = append(ch, put(p, c))
	}
	p, _ := w.propose("seed-"+randHex(3), "Seed", ch...)
	w.land(p)
}

func (w *world) task(title, desc string, scope []string, deps ...string) *agwp.Task {
	w.t.Helper()
	tk, err := w.alice.CreateTask(context.Background(), w.repo, title, desc, scope, deps)
	if err != nil {
		w.t.Fatalf("create task %q: %v", title, err)
	}
	return tk
}

func (w *world) getTask(id string) *agwp.Task {
	w.t.Helper()
	tk, err := w.alice.GetTask(context.Background(), w.repo, id)
	if err != nil {
		w.t.Fatalf("get task %s: %v", id, err)
	}
	return tk
}

func (w *world) proposalsOfTask(id string) []agwp.Proposal {
	w.t.Helper()
	all, err := w.alice.ListProposals(context.Background(), w.repo, agwp.ProposalFilter{})
	if err != nil {
		w.t.Fatalf("list proposals: %v", err)
	}
	var out []agwp.Proposal
	for _, p := range all {
		if p.TaskID == id {
			out = append(out, p)
		}
	}
	return out
}

func (w *world) file(rev, path string) (string, bool) {
	w.t.Helper()
	b, err := w.alice.GetFile(context.Background(), w.repo, rev, path, 1<<20)
	if err != nil {
		if agwp.IsNotFound(err) {
			return "", false
		}
		w.t.Fatalf("read %s@%s: %v", path, rev, err)
	}
	return string(b), true
}

// script is a task description the scripted executor understands.
func script(title string, files map[string]string) string {
	b, _ := json.Marshal(map[string]any{"files": files, "title": title, "message": title})
	return "Scripted change.\nag-script:" + string(b)
}

// ---- the CI worker (the trusted execution plane, played by the test) -------------------

type ciWorker struct {
	w      *world
	issuer string
	key    *agentkey.Key
	token  string
}

// claim takes the next queued job; nil when there is none.
func (c *ciWorker) claim() *agwp.Job {
	c.w.t.Helper()
	code, body := c.w.srv.call(c.token, "POST", "/v1/jobs/claim", map[string]any{"worker": "ci-test", "lease_ms": 60000, "wait_ms": 0})
	if code != 200 {
		c.w.t.Fatalf("claim: %d %s", code, body)
	}
	var out struct {
		Job *agwp.Job `json:"job"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		c.w.t.Fatalf("claim: %v: %s", err, body)
	}
	return out.Job
}

// complete signs and submits the attestation for the exact tree of the job.
func (c *ciWorker) complete(job *agwp.Job, result string) {
	c.w.t.Helper()
	v := job.Validation
	if v == nil {
		c.w.t.Fatalf("job %s carries no validation", job.ID)
	}
	tree, err := hex.DecodeString(v.TreeID)
	if err != nil || len(tree) != 32 {
		c.w.t.Fatalf("tree id %q", v.TreeID)
	}
	id := "att_" + job.ID
	issuedAt := time.Now().UnixMilli()
	resultByte := byte(1)
	if result == "fail" {
		resultByte = 2
	}
	var b []byte
	b = canonStr(b, id)
	b = append(b, tree...)
	b = canonStr(b, v.Check)
	b = append(b, resultByte)
	b = canonVarint(b, v.PolicyVersion)
	b = canonStr(b, "env-test")
	b = canonStr(b, c.issuer)
	b = binary.BigEndian.AppendUint64(b, uint64(issuedAt))
	sig, err := c.key.Sign("attestation.v1", b)
	if err != nil {
		c.w.t.Fatal(err)
	}
	att := map[string]any{
		"id": id, "tree_id": v.TreeID, "check": v.Check, "result": result, "policy_version": v.PolicyVersion,
		"environment_digest": "env-test", "issuer": c.issuer, "issued_at_ms": issuedAt, "public_key": c.key.PublicHex(), "signature": sig,
	}
	code, body := c.w.srv.call(c.token, "POST", "/v1/repos/"+c.w.repo+"/jobs/"+job.ID+"/complete",
		map[string]any{"worker": "ci-test", "attestation": att, "result_json": map[string]any{"exit": 0}})
	if code != 200 {
		c.w.t.Fatalf("complete job %s: %d %s", job.ID, code, body)
	}
}

func canonVarint(b []byte, v uint64) []byte {
	for {
		x := byte(v & 0x7f)
		v >>= 7
		if v == 0 {
			return append(b, x)
		}
		b = append(b, x|0x80)
	}
}

func canonStr(b []byte, s string) []byte {
	b = canonVarint(b, uint64(len(s)))
	return append(b, s...)
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
	t      *testing.T
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
	t := w.t
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
`, w.srv.URL, state, shutdown, oc.Extra, w.repo, oc.Repo, oc.Executors)
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
	env["AG_ORCH_TOKEN"] = w.mint("service:orchestrator", app.OrchestratorOps(cfg))
	env["AG_REVIEWER_TOKEN"] = w.mint("service:reviewer", app.ReviewerOps())
	env["AG_ADVISOR_TOKEN"] = w.mint("service:advisor", app.AdvisorOps())
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

func eventually(t *testing.T, what string, within time.Duration, f func() bool) {
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
func writeScript(t *testing.T, name, body string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, []byte("#!/bin/sh\n"+body), 0o700); err != nil {
		t.Fatal(err)
	}
	return p
}
