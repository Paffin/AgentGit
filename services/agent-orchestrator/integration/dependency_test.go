//go:build linux

package integration

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

const requireProvenanceAndApproval = `{"require_provenance_for_agents": true, "approvals": {"min": 0, "min_for_agents": 1}}`

// testAgentExecutor is the YAML of a command executor that runs the test-only
// agent program (internal/testutil/testagent): it writes the files the task
// description declares, signs provenance and opens a proposal.
func testAgentExecutor(t *testing.T, name string) string {
	t.Helper()
	return fmt.Sprintf(`  - name: %s
    type: command
    command: [%q]
    pass_env: [PATH]
    heartbeat: 1s
    timeout: 2m
    grace: 2s
    env:
      TESTAGENT_TASK_FILES: "1"
      TESTAGENT_MODEL: test-agent
      TESTAGENT_RATIONALE: integration test run
`, name, testutil.BuildTestAgent(t))
}

// Dependency-ordered tasks are run by the test agent through a virtual
// workspace; its provenance is signed with the agent's own key and verified by
// the real server; the proposal is subject to the stream's policy, which the
// orchestrator cannot bypass: only a human approval and a human integrator
// move the stream, and the dependent task starts only after the first landed.
func TestDependencyOrderedTasksLandThroughPolicy(t *testing.T) {
	w := newWorld(t, worldOptions{
		Policy: func(string) string { return requireProvenanceAndApproval },
		Agents: []string{"docs-bot"},
	})
	ctx := context.Background()
	w.Seed(map[string]string{"README.md": "# web\n"})
	a := w.Task("Write A", script("Add a.md", map[string]string{"docs/a.md": "A\n"}), []string{"docs/**"})
	b := w.Task("Write B", script("Add b.md", map[string]string{"docs/b.md": "B\n"}), []string{"docs/**"}, a.ID)

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      default_executor: docs-bot\n",
		Executors: testAgentExecutor(t, "docs-bot"),
	})
	o.waitReady()

	// A runs and proposes; B must not have started.
	eventually(t, "a proposal for task A", 30*time.Second, func() bool { return len(w.ProposalsOfTask(a.ID)) == 1 })
	pa := w.ProposalsOfTask(a.ID)[0]
	if got := w.GetTask(a.ID); got.State != agwp.TaskInProgress || got.Owner != "agent:docs-bot" {
		t.Fatalf("task A = %+v", got)
	}
	if got := w.GetTask(b.ID); got.State != agwp.TaskOpen || got.Ready {
		t.Fatalf("task B must wait for A: %+v", got)
	}
	runs, err := w.Alice.ListRuns(ctx, w.Repo, "", false, 100)
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range runs {
		if r.TaskID == b.ID {
			t.Fatalf("a run for B exists before A landed: %+v", r)
		}
	}

	// The proposal is the agent's, carries verified provenance signed with the agent's key.
	if !strings.HasPrefix(pa.Author, "agent:docs-bot#") {
		t.Fatalf("proposal author = %q", pa.Author)
	}
	prov, err := w.Alice.ListProvenance(ctx, w.Repo, pa.Head)
	if err != nil || len(prov) != 1 {
		t.Fatalf("provenance = %+v, %v", prov, err)
	}
	if !prov[0].Verified || prov[0].Record.TaskID != a.ID || prov[0].Record.Agent != pa.Author || prov[0].Record.Model != "test-agent" {
		t.Fatalf("provenance entry = %+v", prov[0])
	}
	agents, err := w.Alice.ListAgents(ctx, w.Repo)
	if err != nil || len(agents) != 1 || agents[0].Name != "docs-bot" || agents[0].PublicKey != prov[0].PublicKey {
		t.Fatalf("registered agents = %+v, %v (provenance key %s)", agents, err, prov[0].PublicKey)
	}

	// Policy decides, not the orchestrator: without a human approval the integrator is blocked.
	res, err := w.Carol.Integrate(ctx, w.Repo, pa.ID, nil)
	if err != nil {
		t.Fatal(err)
	}
	if res.Outcome.Status != "blocked" || !hasBlocker(res.Outcome.Blockers, agwp.BlockerInsufficientApproval) {
		t.Fatalf("an unapproved agent proposal must be blocked: %+v", res.Outcome)
	}
	if got := w.GetTask(b.ID); got.State != agwp.TaskOpen {
		t.Fatalf("B started although A has not landed: %+v", got)
	}
	if _, err := w.Bob.ReviewProposal(ctx, w.Repo, pa.ID, agwp.DecisionApprove, "looks good"); err != nil {
		t.Fatal(err)
	}
	w.Land(&pa)
	eventually(t, "task A to be finished by its owner", 30*time.Second, func() bool { return w.GetTask(a.ID).State == agwp.TaskDone })

	// Now B becomes ready, runs, and goes through the same gates.
	eventually(t, "a proposal for task B", 30*time.Second, func() bool { return len(w.ProposalsOfTask(b.ID)) == 1 })
	pb := w.ProposalsOfTask(b.ID)[0]
	if _, err := w.Bob.ReviewProposal(ctx, w.Repo, pb.ID, agwp.DecisionApprove, "ok"); err != nil {
		t.Fatal(err)
	}
	w.Land(&pb)
	eventually(t, "task B to be finished", 30*time.Second, func() bool { return w.GetTask(b.ID).State == agwp.TaskDone })

	runs, _ = w.Alice.ListRuns(ctx, w.Repo, "", false, 100)
	var aRun, bRun *agwp.Run
	for i := range runs {
		switch runs[i].TaskID {
		case a.ID:
			aRun = &runs[i]
		case b.ID:
			bRun = &runs[i]
		}
	}
	if aRun == nil || bRun == nil || aRun.Status != "completed" || bRun.Status != "completed" {
		t.Fatalf("runs = %+v", runs)
	}
	if bRun.StartedAtMs < w.GetTask(a.ID).UpdatedAtMs {
		t.Fatalf("B started (%d) before A was finished (%d)", bRun.StartedAtMs, w.GetTask(a.ID).UpdatedAtMs)
	}

	// The landed tree has both files.
	for path, want := range map[string]string{"docs/a.md": "A\n", "docs/b.md": "B\n", "README.md": "# web\n"} {
		if got, ok := w.File("main", path); !ok || got != want {
			t.Errorf("main:%s = %q (%v), want %q", path, got, ok, want)
		}
	}

	// Nothing was started that the server then refused (a regression test: a task finished in a
	// pass was once offered for adoption in the same pass).
	if _, metrics := o.get("/metrics"); strings.Contains(metrics, `reason="start_failed"`) {
		t.Errorf("a session start was attempted and refused:\n%s", o.logs.String())
	}

	// Everything that moved the stream was a human: the audit trail names no service.
	events, _, err := w.Alice.ReplayEvents(ctx, w.Repo, 0, 1000, []string{"stream"})
	if err != nil {
		t.Fatal(err)
	}
	integrations := 0
	for _, e := range events {
		if e.Topic != "stream.integrate" {
			continue
		}
		integrations++
		if e.Payload.Actor != "user:carol" {
			t.Errorf("stream.integrate by %q, want the human integrator", e.Payload.Actor)
		}
	}
	if integrations != 3 { // the seed, A and B
		t.Errorf("%d stream.integrate events, want 3 (seed, A, B)", integrations)
	}
}

func hasBlocker(bs []agwp.Blocker, code string) bool {
	for _, b := range bs {
		if b.Code == code {
			return true
		}
	}
	return false
}
