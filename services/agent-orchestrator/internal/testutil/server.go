// Package testutil is the shared test harness of the orchestrator. It starts
// the REAL ag-server (the binary CI builds with `cargo build -p agentgit-server`) on
// a free loopback port in a temporary data directory, and builds a small world
// on it (a repository, people with roles, a CI worker). No part of the AGWP
// server is imitated anywhere in the test suites.
//
// What is a test double here, and only here:
//
//   - testagent (cmd in ./testagent) is a TEST-ONLY agent program. It is built
//     by BuildTestAgent and run by the orchestrator as an ordinary command
//     executor; it is never part of the shipped binary.
//   - Proxy is a forwarding HTTP proxy in front of the real server. It records
//     requests and can inject faults (a 503, a dropped connection, a cut SSE
//     stream) that a healthy real server cannot be made to produce on demand.
//     Everything it does not break reaches the real server unchanged.
//
// The tests look for the server binary in $AG_SERVER_BIN, then on PATH, then in
// the cargo target directory of the repository (wherever cargo is configured to
// put it). When none exists they print the command that builds it and skip with
// an AG-SKIP marker (components.go), unless AG_REQUIRE_COMPONENTS names
// ag-server-binary (or $CI / $AG_REQUIRE_SERVER is set), in which case they
// fail: CI must never silently skip the real-server tests.
package testutil

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// RandHex returns 2n random hex characters.
func RandHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b)
}

// FindServerBin locates the ag-server binary; "" when there is none. The order: $AG_SERVER_BIN, `ag-server` on
// PATH, then the cargo target directory of this repository, wherever it is: $CARGO_TARGET_DIR, <repo>/target, and
// the one cargo itself reports (`build.target-dir` of a shared ~/.cargo/config.toml). debug before release.
func FindServerBin() string {
	if p := os.Getenv("AG_SERVER_BIN"); p != "" {
		return p
	}
	if p, err := exec.LookPath("ag-server"); err == nil {
		return p
	}
	root := repoRoot()
	var dirs []string
	if d := os.Getenv("CARGO_TARGET_DIR"); d != "" {
		dirs = append(dirs, d)
	}
	if root != "" {
		dirs = append(dirs, filepath.Join(root, "target"))
		if d := cargoTargetDir(root); d != "" {
			dirs = append(dirs, d)
		}
	}
	for _, d := range dirs {
		for _, profile := range []string{"debug", "release"} {
			p := filepath.Join(d, profile, "ag-server")
			if st, err := os.Stat(p); err == nil && !st.IsDir() {
				return p
			}
		}
	}
	return ""
}

// repoRoot is the Cargo workspace this source tree lives in; "" when it is not part of one.
func repoRoot() string {
	_, file, _, _ := runtime.Caller(0)
	for d := filepath.Dir(file); d != filepath.Dir(d); d = filepath.Dir(d) {
		if _, err := os.Stat(filepath.Join(d, "Cargo.toml")); err == nil {
			return d
		}
	}
	return ""
}

var (
	targetOnce sync.Once
	targetDir  string
)

// cargoTargetDir asks cargo where it builds (`cargo metadata`), once; "" when cargo is not there or says nothing.
func cargoTargetDir(root string) string {
	targetOnce.Do(func() {
		cargo, err := exec.LookPath("cargo")
		if err != nil {
			return
		}
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, cargo, "metadata", "--format-version", "1", "--no-deps", "--offline")
		cmd.Dir = root
		out, err := cmd.Output()
		if err != nil {
			return
		}
		var meta struct {
			TargetDirectory string `json:"target_directory"`
		}
		if json.Unmarshal(out, &meta) == nil {
			targetDir = meta.TargetDirectory
		}
	})
	return targetDir
}

// serverBinMissing is what a test says when it found no ag-server: the exact command that builds one.
func serverBinMissing() error {
	build := "cargo build -p agentgit-server"
	if root := repoRoot(); root != "" {
		build = "cd " + root + " && " + build
	}
	return fmt.Errorf("these tests run against the real ag-server and none was found: build it with `%s` (it is then found by itself), "+
		"or set AG_SERVER_BIN=/path/to/ag-server, or put ag-server on PATH", build)
}

// Server is a running ag-server.
type Server struct {
	t       testing.TB
	URL     string
	Token   string // the bootstrap administrator token
	Dir     string
	logPath string
	cmd     *exec.Cmd
	done    chan struct{}
	once    sync.Once
	// Admin is a client with the bootstrap token.
	Admin *agwp.Client
	// CAFile is the PEM CA of a TLS server (ServerOptions.TLS), else "".
	CAFile     string
	hc         *http.Client
	clientOpts []agwp.Option
}

// ServerOptions tunes StartServer.
type ServerOptions struct {
	// Args are extra arguments after `serve`.
	Args []string
	// Env are extra environment variables (KEY=value).
	Env []string
	// DataDir reuses a data directory (a restart of the same server); default: a fresh one.
	DataDir string
	// TLS makes the server speak HTTPS with a throwaway CA and a certificate for 127.0.0.1 (Server.CAFile is the
	// CA in PEM; Server.Client and Server.Admin trust it).
	TLS bool
}

// StartServer runs a fresh ag-server. It is stopped when the test ends,
// whatever the outcome, and also if the test binary dies.
func StartServer(t testing.TB) *Server { return StartServerWith(t, ServerOptions{}) }

// StartServerWith is StartServer with options.
func StartServerWith(t testing.TB, o ServerOptions) *Server {
	t.Helper()
	bin := FindServerBin()
	if bin == "" {
		RequireComponent(t, "ag-server-binary", serverBinMissing())
	}
	dir := t.TempDir()
	token := RandHex(24) // 48 characters; the server wants at least 32
	scheme, caFile := "http", ""
	var httpc *http.Client
	extra := o.Args
	if o.TLS {
		var certFile, keyFile string
		caFile, certFile, keyFile = writeTLSMaterial(t, dir)
		cfgFile := filepath.Join(dir, "server.toml")
		if err := os.WriteFile(cfgFile, []byte(fmt.Sprintf("[server]\ntls_cert = %q\ntls_key = %q\n", certFile, keyFile)), 0o600); err != nil {
			t.Fatal(err)
		}
		extra = append([]string{"--config", cfgFile}, extra...)
		scheme, httpc = "https", trustingClient(t, caFile)
	}
	for attempt := 0; attempt < 5; attempt++ {
		addr := "127.0.0.1:0"
		logPath := filepath.Join(dir, fmt.Sprintf("server-%d.log", attempt))
		lf, err := os.Create(logPath)
		if err != nil {
			t.Fatal(err)
		}
		data := o.DataDir
		if data == "" {
			data = filepath.Join(dir, fmt.Sprintf("data-%d", attempt))
		}
		args := append([]string{"serve", "--data-dir", data, "--listen", addr}, extra...)
		cmd := exec.Command(bin, args...)
		cmd.Env = append([]string{"AG_BOOTSTRAP_TOKEN=" + token, "PATH=" + os.Getenv("PATH"), "HOME=" + dir}, o.Env...)
		// The bound address comes from the listening event, independently of a caller's log filter.
		cmd.Env = append(cmd.Env, "RUST_LOG=info", "NO_COLOR=1")
		cmd.Stdout, cmd.Stderr = lf, lf
		setProcessGroup(cmd)
		if err := cmd.Start(); err != nil {
			t.Fatalf("cannot start %s: %v", bin, err)
		}
		lf.Close()
		hc := &http.Client{Timeout: 30 * time.Second}
		var opts []agwp.Option
		if httpc != nil {
			hc.Transport = httpc.Transport
			opts = append(opts, agwp.WithHTTPClient(httpc), agwp.WithStreamClient(&http.Client{Transport: httpc.Transport}))
		}
		s := &Server{t: t, URL: scheme + "://" + addr, CAFile: caFile, Token: token, Dir: dir, logPath: logPath, cmd: cmd, done: make(chan struct{}), hc: hc, clientOpts: opts}
		go func() { _ = cmd.Wait(); close(s.done) }()
		t.Cleanup(s.Stop)
		bound := s.waitAddress(15 * time.Second)
		if bound == "" {
			s.Stop()
			continue
		}
		s.URL = scheme + "://" + bound
		s.Admin, err = agwp.New(s.URL, secret.New(token), append([]agwp.Option{agwp.WithIdentityLabel("test-admin"), agwp.WithTimeout(30 * time.Second)}, opts...)...)
		if err != nil {
			t.Fatal(err)
		}
		if s.waitReady(15 * time.Second) {
			return s
		}
		s.Stop()
	}
	t.Fatal("the server did not start in five attempts")
	return nil
}

var loopbackAddress = regexp.MustCompile(`127\.0\.0\.1:[0-9]+`)

// Only consume complete listening events: a concurrent log write may end part-way through a port.
func listeningAddress(log []byte) string {
	lines := strings.Split(string(log), "\n")
	for _, line := range lines[:len(lines)-1] {
		if strings.Contains(line, "listening (") {
			if addr := loopbackAddress.FindString(line); addr != "" {
				return addr
			}
		}
	}
	return ""
}

func (s *Server) waitAddress(d time.Duration) string {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		select {
		case <-s.done:
			return ""
		default:
		}
		if log, err := os.ReadFile(s.logPath); err == nil {
			if addr := listeningAddress(log); addr != "" {
				return addr
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	return ""
}

func (s *Server) waitReady(d time.Duration) bool {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		select {
		case <-s.done:
			return false
		default:
		}
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		_, err := s.Admin.Whoami(ctx)
		cancel()
		if err == nil {
			return true
		}
		time.Sleep(50 * time.Millisecond)
	}
	return false
}

// Stop ends the server (SIGTERM, then SIGKILL) and, when the test failed,
// logs the tail of its output. Calling it twice is fine.
func (s *Server) Stop() {
	s.once.Do(func() {
		if s.cmd.Process != nil {
			select {
			case <-s.done:
			default:
				terminateGroup(s.cmd.Process.Pid)
				select {
				case <-s.done:
				case <-time.After(3 * time.Second):
					killGroup(s.cmd.Process.Pid)
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

// Call is a raw HTTP call for what the typed client does not cover (the
// execution-plane worker routes, negative tests with borrowed credentials).
func (s *Server) Call(token, method, path string, body any, headers ...string) (int, []byte) {
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

// Credentials lists the credentials of a subject ("" for everybody), revoked
// ones included, as the server's administrator sees them (never the secret).
func (s *Server) Credentials(subject string) []agwp.Credential {
	s.t.Helper()
	q := "/v1/credentials?include_revoked=true"
	if subject != "" {
		q += "&subject=" + url.QueryEscape(subject)
	}
	code, body := s.Call(s.Token, "GET", q, nil)
	if code != 200 {
		s.t.Fatalf("list credentials: %d %s", code, body)
	}
	var out struct {
		Credentials []agwp.Credential `json:"credentials"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		s.t.Fatalf("list credentials: %v: %s", err, body)
	}
	return out.Credentials
}

// Client is a typed client with the given credential.
func (s *Server) Client(token, label string, opts ...agwp.Option) *agwp.Client {
	s.t.Helper()
	return s.ClientAt(s.URL, token, label, opts...)
}

// ClientAt is Client for another base URL (a Proxy in front of the server).
func (s *Server) ClientAt(base, token, label string, opts ...agwp.Option) *agwp.Client {
	s.t.Helper()
	all := append([]agwp.Option{agwp.WithIdentityLabel(label), agwp.WithTimeout(30 * time.Second)}, s.clientOpts...)
	all = append(all, opts...)
	c, err := agwp.New(base, secret.New(token), all...)
	if err != nil {
		s.t.Fatal(err)
	}
	return c
}
