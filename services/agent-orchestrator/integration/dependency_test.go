//go:build linux

package integration

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

const requireProvenanceAndApproval = `{"require_provenance_for_agents": true, "approvals": {"min": 0, "min_for_agents": 1}}`

const scriptedDocsBot = `  - name: docs-bot
    type: scripted
    heartbeat: 1s
    scripted:
      task_files: true
      model: scripted-demo
      rationale: demonstration run
`

// Dependency-ordered tasks are run by the scripted executor through a virtual
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
	w.seed(map[string]string{"README.md": "# web\n"})
	a := w.task("Write A", script("Add a.md", map[string]string{"docs/a.md": "A\n"}), []string{"docs/**"})
	b := w.task("Write B", script("Add b.md", map[string]string{"docs/b.md": "B\n"}), []string{"docs/**"}, a.ID)

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      default_executor: docs-bot\n",
		Executors: scriptedDocsBot,
	})
	o.waitReady()

	// A runs and proposes; B must not have started.
	eventually(t, "a proposal for task A", 30*time.Second, func() bool { return len(w.proposalsOfTask(a.ID)) == 1 })
	pa := w.proposalsOfTask(a.ID)[0]
	if got := w.getTask(a.ID); got.State != agwp.TaskInProgress || got.Owner != "agent:docs-bot" {
		t.Fatalf("task A = %+v", got)
	}
	if got := w.getTask(b.ID); got.State != agwp.TaskOpen || got.Ready {
		t.Fatalf("task B must wait for A: %+v", got)
	}
	runs, err := w.alice.ListRuns(ctx, w.repo, "", false, 100)
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
	prov, err := w.alice.ListProvenance(ctx, w.repo, pa.Head)
	if err != nil || len(prov) != 1 {
		t.Fatalf("provenance = %+v, %v", prov, err)
	}
	if !prov[0].Verified || prov[0].Record.TaskID != a.ID || prov[0].Record.Agent != pa.Author || prov[0].Record.Model != "scripted-demo" {
		t.Fatalf("provenance entry = %+v", prov[0])
	}
	agents, err := w.alice.ListAgents(ctx, w.repo)
	if err != nil || len(agents) != 1 || agents[0].Name != "docs-bot" || agents[0].PublicKey != prov[0].PublicKey {
		t.Fatalf("registered agents = %+v, %v (provenance key %s)", agents, err, prov[0].PublicKey)
	}

	// Policy decides, not the orchestrator: without a human approval the integrator is blocked.
	res, err := w.carol.Integrate(ctx, w.repo, pa.ID, nil)
	if err != nil {
		t.Fatal(err)
	}
	if res.Outcome.Status != "blocked" || !hasBlocker(res.Outcome.Blockers, agwp.BlockerInsufficientApproval) {
		t.Fatalf("an unapproved agent proposal must be blocked: %+v", res.Outcome)
	}
	if got := w.getTask(b.ID); got.State != agwp.TaskOpen {
		t.Fatalf("B started although A has not landed: %+v", got)
	}
	if _, err := w.bob.ReviewProposal(ctx, w.repo, pa.ID, agwp.DecisionApprove, "looks good"); err != nil {
		t.Fatal(err)
	}
	w.land(&pa)
	eventually(t, "task A to be finished by its owner", 30*time.Second, func() bool { return w.getTask(a.ID).State == agwp.TaskDone })

	// Now B becomes ready, runs, and goes through the same gates.
	eventually(t, "a proposal for task B", 30*time.Second, func() bool { return len(w.proposalsOfTask(b.ID)) == 1 })
	pb := w.proposalsOfTask(b.ID)[0]
	if _, err := w.bob.ReviewProposal(ctx, w.repo, pb.ID, agwp.DecisionApprove, "ok"); err != nil {
		t.Fatal(err)
	}
	w.land(&pb)
	eventually(t, "task B to be finished", 30*time.Second, func() bool { return w.getTask(b.ID).State == agwp.TaskDone })

	runs, _ = w.alice.ListRuns(ctx, w.repo, "", false, 100)
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
	if bRun.StartedAtMs < w.getTask(a.ID).UpdatedAtMs {
		t.Fatalf("B started (%d) before A was finished (%d)", bRun.StartedAtMs, w.getTask(a.ID).UpdatedAtMs)
	}

	// The landed tree has both files.
	for path, want := range map[string]string{"docs/a.md": "A\n", "docs/b.md": "B\n", "README.md": "# web\n"} {
		if got, ok := w.file("main", path); !ok || got != want {
			t.Errorf("main:%s = %q (%v), want %q", path, got, ok, want)
		}
	}

	// Nothing was started that the server then refused (a regression test: a task finished in a
	// pass was once offered for adoption in the same pass).
	if _, metrics := o.get("/metrics"); strings.Contains(metrics, `reason="start_failed"`) {
		t.Errorf("a session start was attempted and refused:\n%s", o.logs.String())
	}

	// Everything that moved the stream was a human: the audit trail names no service.
	events, _, err := w.alice.ReplayEvents(ctx, w.repo, 0, 1000, []string{"stream"})
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
