package app

import (
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
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

// These tests run the whole orchestrator against the real ag-server (internal/testutil).
// The agent is the test-only agent program (internal/testutil/testagent), run as an
// ordinary command executor.

const repoName = testutil.RepoName

// agentExecutor is the YAML of a command executor that runs the program at bin.
func agentExecutor(bin string) string {
	return fmt.Sprintf(`  - name: docs-bot
    type: command
    command: [%q]
    pass_env: [PATH]
    heartbeat: 1s
    timeout: 30s
    env:
      TESTAGENT_FILES: '{"docs/notes.md": "hello\n"}'
      TESTAGENT_TITLE: Add notes
      TESTAGENT_MODEL: test-agent
`, bin)
}

func configDoc(url, state, executors, extra string) string {
	return fmt.Sprintf(`
server:
  url: %s
  timeout: 5s
  retries: 2
state_dir: %s
listen: 127.0.0.1:0
loop:
  reconcile: 1s
  debounce: 10ms
shutdown:
  grace: 5s
repos:
  - name: %s
    schedule:
      slots: 2
executors:
%s%s`, url, state, repoName, executors, extra)
}

// testRig is an App wired to a real ag-server, listening on an ephemeral port.
type testRig struct {
	t    *testing.T
	w    *testutil.World
	app  *App
	cfg  *config.Config
	base string
	mu   sync.Mutex
	envv map[string]string
}

func (r *testRig) getenv(k string) string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.envv[k]
}

func newRig(t *testing.T, mutate func(r *testRig)) *testRig {
	t.Helper()
	r := &testRig{t: t, w: testutil.NewWorld(t, testutil.WorldOptions{Agents: []string{"docs-bot"}}), envv: map[string]string{"AG_ORCH_TOKEN": "placeholder"}}
	w := r.w
	w.Seed(map[string]string{"README.md": "hi\n"})
	cfg, err := config.Parse(strings.NewReader(configDoc(w.Srv.URL, t.TempDir(), agentExecutor(testutil.BuildTestAgent(t)), "")), r.getenv)
	if err != nil {
		t.Fatal(err)
	}
	r.cfg = cfg
	// the orchestrator's credential is scoped to exactly what the configuration needs
	r.envv["AG_ORCH_TOKEN"] = w.Mint("service:orchestrator", OrchestratorOps(cfg))
	r.envv["AG_REVIEWER_TOKEN"] = w.Mint("service:reviewer", ReviewerOps())
	r.envv["AG_ADVISOR_TOKEN"] = w.Mint("service:advisor", AdvisorOps())
	if mutate != nil {
		mutate(r)
	}
	return r
}

func (r *testRig) start(t *testing.T) context.CancelFunc {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var logw io.Writer = io.Discard
	if os.Getenv("AG_TEST_LOG") != "" {
		logw = os.Stderr // for debugging a failing test
	}
	r.app, err = New(Options{Config: r.cfg, Getenv: r.getenv, Log: slog.New(slog.NewTextHandler(logw, nil)), Listener: ln})
	if err != nil {
		t.Fatal(err)
	}
	r.base = "http://" + ln.Addr().String()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- r.app.Run(ctx) }()
	stopped := false
	stop := func() {
		if stopped {
			return
		}
		stopped = true
		cancel()
		select {
		case err := <-done:
			if err != nil {
				t.Errorf("Run returned %v", err)
			}
		case <-time.After(20 * time.Second):
			t.Error("Run did not return after cancellation")
		}
	}
	t.Cleanup(stop)
	return stop
}

func (r *testRig) get(t *testing.T, path, token string) (int, string) {
	t.Helper()
	req, _ := http.NewRequest("GET", r.base+path, nil)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, string(b)
}

func eventually(t *testing.T, what string, within time.Duration, f func() bool) {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if f() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func TestHealthzIsAlwaysUpAndReadyzFollowsTheServer(t *testing.T) {
	r := newRig(t, nil)
	r.start(t)
	if code, _ := r.get(t, "/healthz", ""); code != 200 {
		t.Fatalf("/healthz = %d", code)
	}
	eventually(t, "readyz", 10*time.Second, func() bool {
		code, _ := r.get(t, "/readyz", "")
		return code == 200
	})
}

func TestReadyzStaysUnreadyWithARejectedCredential(t *testing.T) {
	r := newRig(t, func(r *testRig) { r.envv["AG_ORCH_TOKEN"] = "agt_bogus_credential_value" })
	r.start(t)
	time.Sleep(300 * time.Millisecond)
	code, body := r.get(t, "/readyz", "")
	if code != http.StatusServiceUnavailable {
		t.Fatalf("/readyz = %d %s", code, body)
	}
	if !strings.Contains(body, "server:") {
		t.Fatalf("the reason must name the server: %s", body)
	}
	if strings.Contains(body, "agt_bogus_credential_value") {
		t.Fatal("the credential leaked into /readyz")
	}
	if code, _ := r.get(t, "/healthz", ""); code != 200 {
		t.Fatalf("liveness must not depend on the server: %d", code)
	}
}

func TestStatusAndMetricsRequireTheAdminTokenWhenConfigured(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.cfg.Admin.TokenEnv = "AG_ORCH_ADMIN"
		r.envv["AG_ORCH_ADMIN"] = "admin-secret-value-0123456789"
	})
	r.start(t)
	eventually(t, "readyz", 10*time.Second, func() bool { c, _ := r.get(t, "/readyz", ""); return c == 200 })
	for _, p := range []string{"/status", "/metrics", "/index/files?repo=" + repoName, "/advisor/suggestions"} {
		if code, _ := r.get(t, p, ""); code != 401 {
			t.Errorf("%s without a token = %d, want 401", p, code)
		}
		if code, _ := r.get(t, p, "wrong-token-value"); code != 401 {
			t.Errorf("%s with a wrong token = %d, want 401", p, code)
		}
	}
	code, body := r.get(t, "/status", "admin-secret-value-0123456789")
	if code != 200 {
		t.Fatalf("/status = %d %s", code, body)
	}
	for _, secret := range []string{r.envv["AG_ORCH_TOKEN"], "admin-secret-value-0123456789"} {
		if strings.Contains(body, secret) {
			t.Fatal("/status leaked a credential")
		}
	}
	// probes stay open
	if code, _ := r.get(t, "/healthz", ""); code != 200 {
		t.Fatal("/healthz must stay open")
	}
	if code, _ := r.get(t, "/readyz", ""); code != 200 {
		t.Fatal("/readyz must stay open")
	}
	if code, body := r.get(t, "/metrics", "admin-secret-value-0123456789"); code != 200 || !strings.Contains(body, "orchestrator_") {
		t.Fatalf("/metrics = %d\n%s", code, body)
	}
}

func TestStatusDescribesIdentitiesAndRepositories(t *testing.T) {
	r := newRig(t, nil)
	r.start(t)
	eventually(t, "readyz", 10*time.Second, func() bool { c, _ := r.get(t, "/readyz", ""); return c == 200 })
	code, body := r.get(t, "/status", "")
	if code != 200 {
		t.Fatalf("/status = %d", code)
	}
	var st Status
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		t.Fatalf("%v\n%s", err, body)
	}
	if !st.Service.Ready || st.Service.Version != Version {
		t.Fatalf("service = %+v", st.Service)
	}
	if len(st.Identity) == 0 || st.Identity[0].Identity != "service:orchestrator" {
		t.Fatalf("identities = %+v", st.Identity)
	}
	if len(st.Repos) != 1 || st.Repos[0].Name != repoName || st.Repos[0].Scheduler == nil || st.Repos[0].Review == nil || st.Repos[0].Index == nil {
		t.Fatalf("repos = %+v", st.Repos)
	}
	if st.Sessions == nil {
		t.Fatal("sessions must be an empty array, not null")
	}
	if strings.Contains(body, ": null") {
		t.Fatalf("lists must be empty arrays, not null:\n%s", body)
	}
	if _, rbody := r.get(t, "/readyz", ""); strings.Contains(rbody, "null") {
		t.Fatalf("/readyz: %s", rbody)
	}
}

func TestMethodsOtherThanGetAreRefused(t *testing.T) {
	r := newRig(t, nil)
	r.start(t)
	resp, err := http.Post(r.base+"/status", "application/json", strings.NewReader("{}"))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Fatalf("POST /status = %d", resp.StatusCode)
	}
}

func TestATaskFlowsThroughTheWholeWiring(t *testing.T) {
	r := newRig(t, nil)
	task := r.w.Task("Write the notes", "", []string{"docs/**"})
	r.start(t)
	eventually(t, "a proposal for the task", 30*time.Second, func() bool { return len(r.w.ProposalsOfTask(task.ID)) == 1 })
	eventually(t, "the run to complete", 20*time.Second, func() bool {
		runs, err := r.w.Alice.ListRuns(context.Background(), repoName, "", false, 10)
		return err == nil && len(runs) == 1 && runs[0].Status == "completed"
	})
	if got := r.w.GetTask(task.ID); got.Owner != "agent:docs-bot" {
		t.Fatalf("task = %+v", got)
	}
	// the agent's credential was narrow and is revoked again
	eventually(t, "the agent credential to be revoked", 10*time.Second, func() bool {
		minted := 0
		for _, c := range r.w.Srv.Credentials("agent:docs-bot") {
			minted++
			if c.RevokedAtMs == 0 {
				return false
			}
			for _, op := range c.Scope.Operations {
				if config.ForbiddenAgentOps[op] {
					t.Errorf("agent credential carries %s", op)
				}
			}
		}
		return minted > 0
	})
}

// The comment names the identity the reviewer credential really has, not a name assumed in the code.
func TestReviewCommentsNameTheRealReviewerIdentity(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		if err := r.w.Srv.Admin.GrantRole(context.Background(), "service:code-review", repoName, "reviewer"); err != nil {
			t.Fatal(err)
		}
		r.envv["AG_REVIEWER_TOKEN"] = r.w.Mint("service:code-review", ReviewerOps())
	})
	p, _ := r.w.Propose("ws-secret", "plants a key", testutil.Put("pkg/k.go", "package pkg\n\nvar k = \""+"AKIA"+"QWERTYUIOPASDFGH"+"\"\n"))
	r.start(t)
	var body string
	eventually(t, "the review comment", 30*time.Second, func() bool {
		d, err := r.w.Alice.GetProposal(context.Background(), repoName, p.ID)
		if err != nil || len(d.Reviews) == 0 {
			return false
		}
		body = d.Reviews[0].Body
		return true
	})
	if !strings.Contains(body, "(service:code-review)") || strings.Contains(body, "(service:reviewer)") {
		t.Fatalf("the comment does not name its real author:\n%s", body)
	}
	if d, _ := r.w.Alice.GetProposal(context.Background(), repoName, p.ID); d.Reviews[0].Reviewer != "service:code-review" {
		t.Fatalf("review = %+v", d.Reviews[0])
	}
}

func TestShutdownIsGracefulAndClosesTheListener(t *testing.T) {
	r := newRig(t, nil)
	stop := r.start(t)
	eventually(t, "readyz", 10*time.Second, func() bool { c, _ := r.get(t, "/readyz", ""); return c == 200 })
	addr := r.app.Addr()
	stop()
	if c, err := net.DialTimeout("tcp", addr, 500*time.Millisecond); err == nil {
		c.Close()
		t.Fatal("the admin listener is still open after shutdown")
	}
}

// server.ca_file: a real ag-server with a certificate from a private CA is reachable only when the CA is configured.
func TestCAFileMakesAPrivateHTTPSServerReachable(t *testing.T) {
	srv := testutil.StartServerWith(t, testutil.ServerOptions{TLS: true})
	dir := t.TempDir()
	build := func(ca string) (*App, error) {
		getenv := func(k string) string {
			if k == "AG_ORCH_TOKEN" {
				return srv.Token
			}
			return ""
		}
		cfg, err := config.Parse(strings.NewReader(configDoc(srv.URL, dir, agentExecutor("/bin/true"), "")), getenv)
		if err != nil {
			t.Fatal(err)
		}
		cfg.Server.CAFile = ca
		return New(Options{Config: cfg, Getenv: getenv, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	}
	trusted, err := build(srv.CAFile)
	if err != nil {
		t.Fatal(err)
	}
	if caps, err := trusted.orch.Negotiate(context.Background()); err != nil || !caps.Has("events.replay") {
		t.Fatalf("with the CA configured: %v %+v", err, caps)
	}
	if p, err := trusted.orch.Whoami(context.Background()); err != nil || p.Identity == "" {
		t.Fatalf("an authenticated call over TLS: %v %+v", err, p)
	}
	untrusted, err := build("")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := untrusted.orch.Negotiate(context.Background()); err == nil {
		t.Fatal("a certificate from an unknown CA must be refused")
	}
	if _, err := build(filepath.Join(dir, "missing.pem")); err == nil || !strings.Contains(err.Error(), "ca_file") {
		t.Fatalf("a missing CA file must be reported: %v", err)
	}
	bad := filepath.Join(dir, "bad.pem")
	if err := os.WriteFile(bad, []byte("not a certificate"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := build(bad); err == nil || !strings.Contains(err.Error(), "PEM") {
		t.Fatalf("a CA file without a certificate must be reported: %v", err)
	}
}

// ---- check ---------------------------------------------------------------------------

func newCheckApp(t *testing.T, r *testRig) *App {
	t.Helper()
	a, err := New(Options{Config: r.cfg, Getenv: r.getenv, Log: slog.New(slog.NewTextHandler(io.Discard, nil))})
	if err != nil {
		t.Fatal(err)
	}
	return a
}

func (rep CheckReport) find(level, substr string) bool {
	for _, i := range rep.Items {
		if i.Level == level && strings.Contains(i.Subject+" "+i.Message, substr) {
			return true
		}
	}
	return false
}

func TestCheckPassesForAScopedAdminCredential(t *testing.T) {
	r := newRig(t, nil)
	rep := newCheckApp(t, r).Check(context.Background())
	if !rep.OK() {
		t.Fatalf("check failed:\n%s", rep)
	}
	if !rep.find("ok", "covers the") {
		t.Fatalf("the scope should be reported as sufficient:\n%s", rep)
	}
	if !rep.find("ok", "stream \"main\"") {
		t.Fatalf("the stream should be reported:\n%s", rep)
	}
	if strings.Contains(rep.String(), r.envv["AG_ORCH_TOKEN"]) {
		t.Fatal("the report leaks a credential")
	}
}

func TestCheckExplainsAFallbackToTheOrchestratorCredential(t *testing.T) {
	r := newRig(t, func(r *testRig) { delete(r.envv, "AG_REVIEWER_TOKEN") })
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "AG_REVIEWER_TOKEN is not set") {
		t.Fatalf("the orchestrator's own narrow credential cannot post reviews, and the report must say why:\n%s", rep)
	}
}

func TestCheckFlagsAScopeThatLacksOperations(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.envv["AG_ORCH_TOKEN"] = r.w.Mint("service:orchestrator", []string{"repo.read", "events.read"})
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "lacks: ") || !rep.find("error", "credentials.delegate") {
		t.Fatalf("a credential that cannot mint must be an error:\n%s", rep)
	}
}

func TestCheckWarnsAboutAnUnrestrictedCredential(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.envv["AG_ORCH_TOKEN"] = r.w.Mint("service:orchestrator", nil)
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if !rep.OK() || !rep.find("warn", "unrestricted") {
		t.Fatalf("an unrestricted credential is a warning, not an error:\n%s", rep)
	}
}

func TestCheckRequiresTheDelegatorRoleOnEveryRepositoryAndNoMore(t *testing.T) {
	// A writer cannot mint credentials for agents or operate their tasks: reported.
	r := newRig(t, func(r *testRig) {
		if err := r.w.Srv.Admin.GrantRole(context.Background(), "service:writer-orch", repoName, "writer"); err != nil {
			t.Fatal(err)
		}
		r.envv["AG_ORCH_TOKEN"] = r.w.Mint("service:writer-orch", OrchestratorOps(r.cfg))
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "no delegator or admin role on "+repoName) {
		t.Fatalf("missing role must be reported:\n%s", rep)
	}
	// The delegator role is enough: the orchestrator does not need to be an administrator.
	ok := newRig(t, nil)
	if rep := newCheckApp(t, ok).Check(context.Background()); !rep.OK() || rep.find("error", "role") {
		t.Fatalf("a delegator must pass:\n%s", rep)
	}
}

func TestCheckRefusesAServerThatCannotBindCredentialsToRuns(t *testing.T) {
	r := newRig(t, nil)
	// An older server version answers the capability negotiation without the run-bound credential features. The proxy
	// forwards every request to the real server and edits only that answer.
	px := testutil.NewProxy(t, r.w.Srv.URL)
	px.RewriteJSON("/v1/capabilities", func(o map[string]any) {
		var keep []any
		for _, f := range o["features"].([]any) {
			switch f {
			case "auth.run_credentials", "auth.delegation", "agents.operate":
			default:
				keep = append(keep, f)
			}
		}
		o["features"] = keep
	})
	r.cfg.Server.URL = px.URL
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "run-bound credentials are not advertised") || !rep.find("error", "delegator role") {
		t.Fatalf("an old server must be an error, not a silent downgrade:\n%s", rep)
	}
}

func TestCheckReportsAnUnreachableServer(t *testing.T) {
	r := newRig(t, nil)
	r.w.Srv.Stop()
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "server") {
		t.Fatalf("an unreachable server must fail the check:\n%s", rep)
	}
}

func TestCheckReportsARejectedCredential(t *testing.T) {
	r := newRig(t, func(r *testRig) { r.envv["AG_ORCH_TOKEN"] = "agt_nope_nope_nope" })
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "credential rejected") {
		t.Fatalf("%s", rep)
	}
}

func TestSessionsOfOneAgentRunningSideBySideNeedNoWarning(t *testing.T) {
	// Every session has a credential bound to its run, so slots above 1 are as safe as slots: 1.
	r := newRig(t, func(r *testRig) { r.cfg.Execs[0].Slots = 3 })
	rep := newCheckApp(t, r).Check(context.Background())
	if !rep.OK() || rep.find("warn", "share one server identity") || rep.find("warn", "act as each other") {
		t.Fatalf("%s", rep)
	}
}

func TestCheckWarnsWhenACommandIsMissing(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.cfg.Execs = append(r.cfg.Execs, config.Executor{Name: "ghost", Type: "command", Agent: "ghost", Command: []string{"/nonexistent/agent-binary"}})
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if !rep.find("warn", "not found") {
		t.Fatalf("%s", rep)
	}
}

// ---- requirements --------------------------------------------------------------------

func TestOrchestratorOpsNeverCarryTheIntegrationPowers(t *testing.T) {
	cfg := minimalConfig(t)
	ops := map[string]bool{}
	for _, o := range OrchestratorOps(cfg) {
		ops[o] = true
	}
	for _, forbidden := range []string{"integrate.run", "proposal.review", "proposal.close", "policy.write", "stream.cas", "stream.force", "gc.run", "roles.manage", "queue.run", "attestation.submit", "jobs.claim", "repo.create"} {
		if ops[forbidden] {
			t.Errorf("the orchestrator's operations include %q", forbidden)
		}
	}
	for _, need := range []string{"credentials.delegate", "agent.operate", "agent.register", "agent.run", "integrate.prepare", "integrate.validate", "events.read"} {
		if !ops[need] {
			t.Errorf("the orchestrator's operations lack %q", need)
		}
	}
	// It needs no administrator power over credentials: it mints for agents only.
	for _, admin := range []string{"credentials.mint", "credentials.revoke", "credentials.list"} {
		if ops[admin] {
			t.Errorf("the orchestrator's operations include the administrator's %q", admin)
		}
	}
}

func TestIntegrateValidateIsOnlyRequestedWhenEnqueueingIsOn(t *testing.T) {
	cfg := minimalConfig(t)
	off := false
	cfg.Repos[0].ExecSched.Enqueue = &off
	for _, o := range OrchestratorOps(cfg) {
		if o == "integrate.validate" {
			t.Fatal("integrate.validate must not be requested when enqueueing is off")
		}
	}
}

func TestEveryAgentOperationIsContainedInTheOrchestratorScope(t *testing.T) {
	// the server refuses to mint a credential wider than the minting one, so
	// the orchestrator's scope has to contain whatever it hands out
	cfg := minimalConfig(t)
	cfg.Execs[0].Ops = append(cfg.Execs[0].Ops, "stream.read", "tree.read")
	have := map[string]bool{}
	for _, o := range OrchestratorOps(cfg) {
		have[o] = true
	}
	for _, e := range cfg.Execs {
		for _, o := range e.Ops {
			if !have[o] {
				t.Errorf("executor %s may be granted %q but the orchestrator's own scope lacks it", e.Name, o)
			}
		}
	}
}

func TestReviewerAndAdvisorOperationsAreMinimal(t *testing.T) {
	for name, ops := range map[string][]string{"reviewer": ReviewerOps(), "advisor": AdvisorOps()} {
		has := map[string]bool{}
		for _, o := range ops {
			has[o] = true
			if config.ForbiddenAgentOps[o] && o != "proposal.review" {
				t.Errorf("%s carries %s", name, o)
			}
		}
		if !has["proposal.review"] || !has["proposal.read"] {
			t.Errorf("%s cannot post reviews: %v", name, ops)
		}
		for _, bad := range []string{"integrate.run", "policy.write", "stream.cas", "credentials.mint", "workspace.write", "proposal.create"} {
			if has[bad] {
				t.Errorf("%s carries %s", name, bad)
			}
		}
	}
}

func minimalConfig(t *testing.T) *config.Config {
	t.Helper()
	cfg, err := config.Parse(strings.NewReader(configDoc("http://127.0.0.1:1", t.TempDir(), agentExecutor("/bin/true"), "")), func(k string) string {
		if k == "AG_ORCH_TOKEN" {
			return "x"
		}
		return ""
	})
	if err != nil {
		t.Fatal(err)
	}
	return cfg
}

func TestHasRole(t *testing.T) {
	cases := []struct {
		roles []string
		repo  string
		want  []string
		ok    bool
	}{
		{[]string{"*:admin"}, "acme/web", []string{"admin"}, true},
		{[]string{"acme/web:admin"}, "acme/web", []string{"admin"}, true},
		{[]string{"acme/*:admin"}, "acme/web", []string{"admin"}, true},
		{[]string{"acme/*:admin"}, "other/web", []string{"admin"}, false},
		{[]string{"acme/web:reviewer"}, "acme/web", []string{"reviewer", "admin"}, true},
		{[]string{"acme/web:writer"}, "acme/web", []string{"reviewer", "admin"}, false},
		{[]string{"garbage"}, "acme/web", []string{"admin"}, false},
		{nil, "acme/web", []string{"admin"}, false},
		// the server matches role bindings with the globset crate: ?, inner *, classes and alternatives are valid
		{[]string{"acme/we?:admin"}, "acme/web", []string{"admin"}, true},
		{[]string{"acme/we?:admin"}, "acme/webb", []string{"admin"}, false},
		{[]string{"*/web:delegator"}, "acme/web", []string{"delegator", "admin"}, true},
		{[]string{"*/web:delegator"}, "acme/api", []string{"delegator", "admin"}, false},
		{[]string{"{acme,other}/web:admin"}, "other/web", []string{"admin"}, true},
		{[]string{"{acme,other}/web:admin"}, "third/web", []string{"admin"}, false},
		{[]string{"acme/[wa]*:admin"}, "acme/api", []string{"admin"}, true},
		{[]string{"acme/[!w]*:admin"}, "acme/web", []string{"admin"}, false},
		{[]string{"acme/a*b*c:admin"}, "acme/abxbc", []string{"admin"}, true},
		{[]string{"acme/web.git:admin"}, "acme/webXgit", []string{"admin"}, false}, // a dot is a dot
	}
	for _, c := range cases {
		if got := hasRole(c.roles, c.repo, c.want); got != c.ok {
			t.Errorf("hasRole(%v, %s, %v) = %v", c.roles, c.repo, c.want, got)
		}
	}
}

// A pattern the local matcher cannot translate is not evidence of a missing role: the check warns instead of failing.
func TestAnUntranslatablePatternIsAWarningNotAMissingRole(t *testing.T) {
	has, unsure := roleCheck([]string{"acme/[web:admin"}, "acme/web", []string{"admin"})
	if has || len(unsure) != 1 {
		t.Fatalf("has=%v unsure=%v", has, unsure)
	}
	r := newRig(t, func(r *testRig) {
		// a binding with a valid pattern that the server accepts: any repository named like a file ending in b
		if err := r.w.Srv.Admin.GrantRole(context.Background(), "service:glob-orch", "*/we?", "delegator"); err != nil {
			t.Fatal(err)
		}
		r.envv["AG_ORCH_TOKEN"] = r.w.Mint("service:glob-orch", OrchestratorOps(r.cfg))
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.find("error", "no delegator or admin role") {
		t.Fatalf("a valid server-side pattern (*/we?) was reported as a missing role:\n%s", rep)
	}
}

func TestCheckReportString(t *testing.T) {
	var rep CheckReport
	rep.add("ok", "server", "fine")
	rep.add("warn", "orchestrator", "careful")
	if !rep.OK() {
		t.Fatal("warnings do not fail a check")
	}
	rep.add("error", "repo", "broken")
	if rep.OK() {
		t.Fatal("an error must fail a check")
	}
	s := rep.String()
	for _, want := range []string{"ok", "WARN", "ERROR", "fine", "careful", "broken"} {
		if !strings.Contains(s, want) {
			t.Errorf("report lacks %q:\n%s", want, s)
		}
	}
}
