package fakeagwp_test

import (
	"context"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

// The fake is test infrastructure, so these tests pin the behaviours other
// packages rely on to be faithful to the real server.

func client(t *testing.T, f *fakeagwp.Server, token string) *agwp.Client {
	t.Helper()
	c, err := agwp.New(f.URL, secret.New(token), agwp.WithRetry(agwp.RetryPolicy{MaxAttempts: 1}))
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func TestErrorModelMatchesTheProtocol(t *testing.T) {
	f := fakeagwp.New()
	defer f.Close()
	f.AddRepo("acme/web")
	reader := f.AddToken(fakeagwp.TokenSpec{Identity: "user:r", Role: "reader"})
	ctx := context.Background()

	if _, err := client(t, f, "agt_nope").Whoami(ctx); !agwp.IsCode(err, agwp.CodeUnauthenticated) {
		t.Fatalf("bad token: %v", err)
	}
	if _, err := client(t, f, reader).ListTasks(ctx, "acme/missing", "", false); !agwp.IsNotFound(err) {
		t.Fatalf("unknown repo: %v", err)
	}
	if _, err := client(t, f, reader).CreateTask(ctx, "acme/web", "t", "", nil, nil); !agwp.IsCode(err, agwp.CodePermissionDenied) {
		t.Fatalf("a reader may not write: %v", err)
	}
	e, _ := agwp.AsError(func() error { _, err := client(t, f, reader).GetTask(ctx, "acme/web", "task_x"); return err }())
	if e == nil || e.RequestID == "" || e.HTTPStatus != 404 {
		t.Fatalf("error = %+v", e)
	}
}

func TestInjectedFailuresAreHonouredInOrder(t *testing.T) {
	f := fakeagwp.New()
	defer f.Close()
	f.AddRepo("acme/web")
	tok := f.AddToken(fakeagwp.TokenSpec{Identity: "user:w"})
	f.FailNext("task.list", 2, 429, "rate_limited", true, 1500)
	c := client(t, f, tok)
	for i := 0; i < 2; i++ {
		_, err := c.ListTasks(context.Background(), "acme/web", "", false)
		e, ok := agwp.AsError(err)
		if !ok || e.Code != agwp.CodeRateLimited || !e.Retryable || e.RetryAfter != 1500*time.Millisecond {
			t.Fatalf("attempt %d: %v", i, err)
		}
	}
	if _, err := c.ListTasks(context.Background(), "acme/web", "", false); err != nil {
		t.Fatalf("injection must be exhausted: %v", err)
	}
	f.DropConnectionNext("task.list", 2) // net/http transparently retries an idempotent GET once on a reused connection
	if _, err := c.ListTasks(context.Background(), "acme/web", "", false); err == nil {
		t.Fatal("expected a dropped connection")
	}
	f.ClearFailures()
	if _, err := c.ListTasks(context.Background(), "acme/web", "", false); err != nil {
		t.Fatal(err)
	}
}

func TestCredentialsCanOnlyNarrowAndExpire(t *testing.T) {
	f := fakeagwp.New()
	defer f.Close()
	f.AddRepo("acme/web")
	parent := f.AddToken(fakeagwp.TokenSpec{Identity: "service:orch", Role: "admin", Ops: []string{"credentials.mint", "credentials.revoke", "task.read"}, Repos: []string{"acme/*"}})
	c := client(t, f, parent)
	ctx := context.Background()
	if _, err := c.MintCredential(ctx, agwp.MintRequest{Subject: "agent:a", TTLSecs: 60, Scope: agwp.CredentialScope{Repos: []string{"acme/web"}, Operations: []string{"task.read"}}}); err != nil {
		t.Fatalf("narrowing must work: %v", err)
	}
	for name, sc := range map[string]agwp.CredentialScope{
		"wider operations": {Repos: []string{"acme/web"}, Operations: []string{"task.read", "task.write"}},
		"no operations":    {Repos: []string{"acme/web"}},
		"other repo":       {Repos: []string{"other/web"}, Operations: []string{"task.read"}},
	} {
		if _, err := c.MintCredential(ctx, agwp.MintRequest{Subject: "agent:a", TTLSecs: 60, Scope: sc}); !agwp.IsCode(err, agwp.CodePermissionDenied) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	if _, err := c.MintCredential(ctx, agwp.MintRequest{Subject: "agent:a", TTLSecs: 7200, Scope: agwp.CredentialScope{Repos: []string{"acme/web"}, Operations: []string{"task.read"}}}); !agwp.IsCode(err, agwp.CodeInvalidArgument) {
		t.Errorf("agent credentials last at most an hour: %v", err)
	}
	m, _ := c.MintCredential(ctx, agwp.MintRequest{Subject: "agent:a", TTLSecs: 60, Scope: agwp.CredentialScope{Repos: []string{"acme/web"}, Operations: []string{"task.read"}}})
	child := client(t, f, m.Token.Reveal())
	if _, err := child.ListTasks(ctx, "acme/web", "", false); err != nil {
		t.Fatal(err)
	}
	if _, err := child.ListLeases(ctx, "acme/web", false); !agwp.IsCode(err, agwp.CodePermissionDenied) {
		t.Fatalf("a scoped credential must not reach other operations: %v", err)
	}
	if err := c.RevokeCredential(ctx, m.Credential.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := child.ListTasks(ctx, "acme/web", "", false); !agwp.IsCode(err, agwp.CodeUnauthenticated) {
		t.Fatalf("a revoked credential must stop working: %v", err)
	}
}

func TestTaskOwnershipFollowsTheServer(t *testing.T) {
	f := fakeagwp.New()
	defer f.Close()
	f.AddRepo("acme/web")
	svc := f.AddToken(fakeagwp.TokenSpec{Identity: "service:orch", Role: "writer"})
	operator := f.AddToken(fakeagwp.TokenSpec{Identity: "service:operator", Role: "delegator"})
	agent := f.AddToken(fakeagwp.TokenSpec{Identity: "agent:bot"})
	ctx := context.Background()
	c, a, op := client(t, f, svc), client(t, f, agent), client(t, f, operator)
	k := "00000000000000000000000000000000000000000000000000000000000000aa"
	if _, err := a.RegisterAgent(ctx, "acme/web", "bot", k); err != nil {
		t.Fatal(err)
	}
	dep := f.AddTask("acme/web", "dep", nil)
	blocked := f.AddTask("acme/web", "blocked", nil, dep)
	if _, err := c.StartRun(ctx, "acme/web", "bot", blocked); !agwp.IsCode(err, agwp.CodeFailedPrecond) {
		t.Fatalf("a blocked task cannot be started: %v", err)
	}
	run, err := c.StartRun(ctx, "acme/web", "bot", dep)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.Task("acme/web", dep); got.Owner != "agent:bot" || got.State != agwp.TaskInProgress {
		t.Fatalf("task = %+v", got)
	}
	if _, err := c.FinishTask(ctx, "acme/web", dep, "done"); !agwp.IsCode(err, agwp.CodeFailedPrecond) {
		t.Fatalf("a service may not finish an agent's task: %v", err)
	}
	// An operator (the delegator role) may, as on the real server; the owner still may too.
	if _, err := op.FinishTask(ctx, "acme/web", dep, "done"); err != nil {
		t.Fatalf("an operator finishes any task: %v", err)
	}
	if !f.Task("acme/web", blocked).Ready {
		t.Fatal("finishing the dependency must unblock the task")
	}
	_ = run
}

func TestEventsReplayAndResume(t *testing.T) {
	f := fakeagwp.New()
	defer f.Close()
	f.AddRepo("acme/web")
	tok := f.AddToken(fakeagwp.TokenSpec{Identity: "user:r", Role: "reader"})
	c := client(t, f, tok)
	f.AddTask("acme/web", "a", nil)
	f.Emit("acme/web", "lease.acquire", "agent:x", "l", nil)
	evs, latest, err := c.ReplayEvents(context.Background(), "acme/web", 0, 0, []string{"task"})
	if err != nil || len(evs) != 1 || evs[0].Topic != "task.create" || latest != f.LatestSeq("acme/web") {
		t.Fatalf("evs=%+v latest=%d err=%v", evs, latest, err)
	}
	evs, _, _ = c.ReplayEvents(context.Background(), "acme/web", evs[0].Seq, 0, nil)
	if len(evs) != 1 || evs[0].Topic != "lease.acquire" {
		t.Fatalf("resume after seq: %+v", evs)
	}
}
