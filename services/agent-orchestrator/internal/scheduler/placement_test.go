package scheduler

import (
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
)

func TestDecide(t *testing.T) {
	task := agwp.Task{ID: "t1", Title: "docs", Scope: []string{"docs/**"}}
	tests := []struct {
		name   string
		mut    func(in *Input)
		defer_ bool
		reason string
	}{
		{"no conflicts", func(in *Input) {}, false, ""},
		{"a task without a scope overlaps nothing", func(in *Input) {
			in.Task.Scope = nil
			in.Leases = []agwp.Lease{{Holder: "agent:other#r", Scope: "**", Exclusive: true}}
		}, false, ""},
		{"foreign exclusive lease", func(in *Input) {
			in.Leases = []agwp.Lease{{Holder: "user:alice", Scope: "docs/api/**", Exclusive: true}}
		}, true, ReasonExclusiveLease},
		{"foreign exclusive lease defers even under allow", func(in *Input) {
			in.Policy = "allow"
			in.Leases = []agwp.Lease{{Holder: "user:alice", Scope: "docs/**", Exclusive: true}}
		}, true, ReasonExclusiveLease},
		{"our own agent's exclusive lease is not foreign", func(in *Input) {
			in.Leases = []agwp.Lease{{Holder: "agent:docs-bot#run_old", Scope: "docs/**", Exclusive: true}}
		}, false, ""},
		{"exclusive lease on a disjoint scope", func(in *Input) {
			in.Leases = []agwp.Lease{{Holder: "user:alice", Scope: "src/**", Exclusive: true}}
		}, false, ""},
		{"foreign soft lease defers under avoid", func(in *Input) {
			in.Leases = []agwp.Lease{{Holder: "agent:other#r", Scope: "docs/**"}}
		}, true, ReasonForeignLease},
		{"foreign soft lease is ignored under allow", func(in *Input) {
			in.Policy = "allow"
			in.Leases = []agwp.Lease{{Holder: "agent:other#r", Scope: "docs/**"}}
		}, false, ""},
		{"running session on an overlapping scope", func(in *Input) {
			in.Running = []session.Info{{RunID: "run_1", TaskID: "t0", Scope: []string{"docs/api/**"}}}
		}, true, ReasonRunningTask},
		{"running session on a disjoint scope", func(in *Input) {
			in.Running = []session.Info{{RunID: "run_1", TaskID: "t0", Scope: []string{"src/**"}}}
		}, false, ""},
		{"a task does not conflict with its own session", func(in *Input) {
			in.Running = []session.Info{{RunID: "run_1", TaskID: "t1", Scope: []string{"docs/**"}}}
		}, false, ""},
		{"running overlap is ignored under allow", func(in *Input) {
			in.Policy = "allow"
			in.Running = []session.Info{{RunID: "run_1", TaskID: "t0", Scope: []string{"docs/**"}}}
		}, false, ""},
		{"finished work that has not landed", func(in *Input) {
			in.Awaiting = []agwp.Task{{ID: "t0", Scope: []string{"docs/guide/**"}}}
		}, true, ReasonPendingLanding},
		{"another task in progress", func(in *Input) {
			in.Tasks = []agwp.Task{{ID: "t0", State: agwp.TaskInProgress, Scope: []string{"docs/x"}}, {ID: "t9", State: agwp.TaskOpen, Scope: []string{"docs/**"}}}
		}, true, ReasonTaskInProgress},
		{"open tasks do not count", func(in *Input) {
			in.Tasks = []agwp.Task{{ID: "t9", State: agwp.TaskOpen, Scope: []string{"docs/**"}}}
		}, false, ""},
		{"an open proposal already touches the scope", func(in *Input) {
			in.OpenProposals = []agwp.Proposal{{ID: "prop_1", TaskID: "t7"}}
			in.ProposalPaths = map[string][]string{"prop_1": {"README.md", "docs/index.md"}}
		}, true, ReasonOpenProposal},
		{"our own task's proposal does not block it", func(in *Input) {
			in.OpenProposals = []agwp.Proposal{{ID: "prop_1", TaskID: "t1"}}
			in.ProposalPaths = map[string][]string{"prop_1": {"docs/index.md"}}
		}, false, ""},
		{"open proposal elsewhere", func(in *Input) {
			in.OpenProposals = []agwp.Proposal{{ID: "prop_1"}}
			in.ProposalPaths = map[string][]string{"prop_1": {"src/main.go"}}
		}, false, ""},
		{"predicted proposal overlap on a file in scope", func(in *Input) {
			in.Overlaps = []agwp.Overlap{{Kind: "proposals_touch_same_paths", Paths: []string{"docs/a.md"}}}
		}, true, ReasonPredicted},
		{"predicted lease overlap on scopes", func(in *Input) {
			in.Overlaps = []agwp.Overlap{{Kind: "leases_overlap", Paths: []string{"docs/**", "docs/a/**"}}}
		}, true, ReasonPredicted},
		{"predicted task overlap pairs", func(in *Input) {
			in.Overlaps = []agwp.Overlap{{Kind: "task_scopes_overlap", Paths: []string{"src/** ~ src/a"}}}
		}, false, ""},
		{"prediction elsewhere is ignored", func(in *Input) {
			in.Overlaps = []agwp.Overlap{{Kind: "stream_moved_under_proposal", Paths: []string{"src/main.go"}}}
		}, false, ""},
		{"prediction is ignored under allow", func(in *Input) {
			in.Policy = "allow"
			in.Overlaps = []agwp.Overlap{{Kind: "stream_moved_under_proposal", Paths: []string{"docs/a.md"}}}
		}, false, ""},
	}
	for _, tt := range tests {
		in := Input{Task: task, Policy: "avoid", Agent: "docs-bot"}
		tt.mut(&in)
		got := Decide(in)
		if got.Defer != tt.defer_ || got.Reason != tt.reason {
			t.Errorf("%s: Decide = %+v, want defer=%v reason=%q", tt.name, got, tt.defer_, tt.reason)
		}
		if got.Defer && got.Detail == "" {
			t.Errorf("%s: a deferral must explain itself", tt.name)
		}
	}
}

func TestDecidePrecedence(t *testing.T) {
	// An exclusive lease is reported before softer reasons.
	in := Input{
		Task: agwp.Task{ID: "t1", Scope: []string{"a/**"}}, Policy: "avoid", Agent: "bot",
		Leases:  []agwp.Lease{{Holder: "agent:x#1", Scope: "a/**"}, {Holder: "user:u", Scope: "a/b", Exclusive: true}},
		Running: []session.Info{{TaskID: "t0", Scope: []string{"a/**"}}},
	}
	if got := Decide(in); got.Reason != ReasonExclusiveLease {
		t.Fatalf("got %+v", got)
	}
}

func TestPrincipal(t *testing.T) {
	if principal("agent:a#run_1") != "agent:a" || principal("user:x") != "user:x" {
		t.Fatal("principal")
	}
}
