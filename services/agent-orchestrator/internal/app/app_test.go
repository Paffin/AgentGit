package app

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repoName = "acme/web"

func configDoc(url, state string, extra string) string {
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
  - name: docs-bot
    type: scripted
    heartbeat: 1s
    timeout: 30s
    scripted:
      files:
        docs/notes.md: "hello\n"
      title: Add notes
%s`, url, state, repoName, extra)
}

// testRig is an App wired to a fake AGWP server, listening on an ephemeral port.
type testRig struct {
	t    *testing.T
	fake *fakeagwp.Server
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
	r := &testRig{t: t, fake: fakeagwp.New(), envv: map[string]string{"AG_ORCH_TOKEN": "placeholder"}}
	t.Cleanup(r.fake.Close)
	r.fake.AddRepo(repoName)
	r.fake.SetHead(repoName, "main", r.fake.Commit(repoName, nil, map[string]string{"README.md": "hi\n"}, "user:root", "seed"))
	cfg, err := config.Parse(strings.NewReader(configDoc(r.fake.URL, t.TempDir(), "")), r.getenv)
	if err != nil {
		t.Fatal(err)
	}
	r.cfg = cfg
	// the orchestrator's credential is scoped to exactly what the configuration needs
	r.envv["AG_ORCH_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin", Ops: OrchestratorOps(cfg)})
	r.envv["AG_REVIEWER_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:reviewer", Role: "reviewer", Ops: ReviewerOps()})
	r.envv["AG_ADVISOR_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:advisor", Role: "reviewer", Ops: AdvisorOps()})
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
	r.app, err = New(Options{Config: r.cfg, Getenv: r.getenv, Log: slog.New(slog.NewTextHandler(io.Discard, nil)), Listener: ln})
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
	task := r.fake.AddTask(repoName, "Write the notes", []string{"docs/**"})
	r.start(t)
	eventually(t, "a proposal for the task", 20*time.Second, func() bool {
		for _, p := range r.fake.Proposals(repoName) {
			if p.TaskID == task {
				return true
			}
		}
		return false
	})
	eventually(t, "the run to complete", 10*time.Second, func() bool {
		runs := r.fake.Runs(repoName)
		return len(runs) == 1 && runs[0].Status == "completed"
	})
	if got := r.fake.Task(repoName, task); got.Owner != "agent:docs-bot" {
		t.Fatalf("task = %+v", got)
	}
	// the agent's credential was narrow and is revoked again
	eventually(t, "the agent credential to be revoked", 10*time.Second, func() bool {
		minted := 0
		for _, m := range r.fake.MintedTokens() {
			if m.Identity != "agent:docs-bot" {
				continue
			}
			minted++
			if !m.Revoked {
				return false
			}
			for _, op := range m.Ops {
				if config.ForbiddenAgentOps[op] {
					t.Errorf("agent credential carries %s", op)
				}
			}
		}
		return minted > 0
	})
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
		r.envv["AG_ORCH_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin", Ops: []string{"repo.read", "events.read"}})
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "lacks: ") || !rep.find("error", "credentials.mint") {
		t.Fatalf("a credential that cannot mint must be an error:\n%s", rep)
	}
}

func TestCheckWarnsAboutAnUnrestrictedCredential(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.envv["AG_ORCH_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin"})
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if !rep.OK() || !rep.find("warn", "unrestricted") {
		t.Fatalf("an unrestricted credential is a warning, not an error:\n%s", rep)
	}
}

func TestCheckRequiresTheAdminRoleOnEveryRepository(t *testing.T) {
	r := newRig(t, func(r *testRig) {
		r.envv["AG_ORCH_TOKEN"] = r.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "writer", Ops: OrchestratorOps(r.cfg)})
	})
	rep := newCheckApp(t, r).Check(context.Background())
	if rep.OK() || !rep.find("error", "no admin role on "+repoName) {
		t.Fatalf("missing role must be reported:\n%s", rep)
	}
}

func TestCheckReportsAnUnreachableServer(t *testing.T) {
	r := newRig(t, nil)
	r.fake.Close()
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
	for _, need := range []string{"credentials.mint", "credentials.revoke", "agent.register", "agent.run", "integrate.prepare", "integrate.validate", "events.read"} {
		if !ops[need] {
			t.Errorf("the orchestrator's operations lack %q", need)
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
	cfg, err := config.Parse(strings.NewReader(configDoc("http://127.0.0.1:1", t.TempDir(), "")), func(k string) string {
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
	}
	for _, c := range cases {
		if got := hasRole(c.roles, c.repo, c.want); got != c.ok {
			t.Errorf("hasRole(%v, %s, %v) = %v", c.roles, c.repo, c.want, got)
		}
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
