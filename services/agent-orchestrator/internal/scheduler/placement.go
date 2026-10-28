package scheduler

import (
	"fmt"
	"strings"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
)

// Deferral reasons (stable strings: metrics labels and status output).
const (
	ReasonOperatorPause      = "operator_pause"      // repository operator paused new task launches
	ReasonExclusiveLease     = "exclusive_lease"     // someone else holds an exclusive lease on an overlapping scope
	ReasonForeignLease       = "foreign_lease"       // someone else holds a soft lease on an overlapping scope
	ReasonRunningTask        = "running_task"        // one of our sessions works on an overlapping scope
	ReasonPendingLanding     = "pending_integration" // finished work on an overlapping scope has not landed yet
	ReasonTaskInProgress     = "task_in_progress"    // another task with an overlapping scope is in progress
	ReasonOpenProposal       = "open_proposal"       // an open proposal already touches the scope
	ReasonPredicted          = "predicted_overlap"   // the server's conflict prediction reports an overlap
	ReasonExecutorBusy       = "executor_busy"       // no free slot (not a conflict)
	ReasonRepoBusy           = "repo_busy"           // the repository's session limit is reached (not a conflict)
	ReasonRetryBackoff       = "retry_backoff"       // waiting out the delay after a failed attempt
	ReasonLeaseConflict      = "lease_conflict"      // the server refused the lease at start
	ReasonStartFailed        = "start_failed"        // starting the session failed; retried after a delay
	ReasonPredictUnavailable = "predict_unavailable" // the conflict prediction cannot be read and on_predict_error is defer
)

// Input is what a placement decision looks at. All of it is advisory data
// read from the server; the decision only chooses *when* to start work, it
// never grants or denies anything.
type Input struct {
	Task    agwp.Task
	Policy  string // "avoid" or "allow"
	Agent   string // the agent that would run the task
	Leases  []agwp.Lease
	Tasks   []agwp.Task
	Running []session.Info
	// Awaiting lists tasks whose work was proposed but has not landed.
	Awaiting []agwp.Task
	// OpenProposals with the files each one changes (from the last prepare).
	OpenProposals []agwp.Proposal
	ProposalPaths map[string][]string
	Overlaps      []agwp.Overlap
}

// Decision is the verdict for one task.
type Decision struct {
	Defer  bool
	Reason string
	Detail string
}

func start() Decision { return Decision{} }

func deferFor(reason, format string, a ...any) Decision {
	return Decision{Defer: true, Reason: reason, Detail: fmt.Sprintf(format, a...)}
}

func principal(identity string) string {
	p, _, _ := strings.Cut(identity, "#")
	return p
}

// Decide applies the placement rules in order; the first that matches wins.
//
//  1. An exclusive lease held by someone else on an overlapping scope always
//     defers: the server would refuse to integrate our work into that scope.
//  2. With policy "allow" nothing else defers.
//  3. With policy "avoid" (the default) the task also waits for: soft leases
//     of others, our own running sessions, finished-but-not-landed work, other
//     in-progress tasks, open proposals that already touch the scope, and
//     overlaps the server's predictor reports.
//
// A task without a declared scope overlaps nothing: the orchestrator cannot
// protect what it does not know about.
func Decide(in Input) Decision {
	scope := in.Task.Scope
	if len(scope) == 0 {
		return start()
	}
	self := "agent:" + in.Agent
	for _, l := range in.Leases {
		if l.Exclusive && principal(l.Holder) != self && glob.AnyOverlap(scope, []string{l.Scope}) {
			return deferFor(ReasonExclusiveLease, "%s holds an exclusive lease on %q", l.Holder, l.Scope)
		}
	}
	if in.Policy == "allow" {
		return start()
	}
	for _, l := range in.Leases {
		if !l.Exclusive && principal(l.Holder) != self && glob.AnyOverlap(scope, []string{l.Scope}) {
			return deferFor(ReasonForeignLease, "%s holds a lease on %q", l.Holder, l.Scope)
		}
	}
	for _, r := range in.Running {
		if r.TaskID != in.Task.ID && glob.AnyOverlap(scope, r.Scope) {
			return deferFor(ReasonRunningTask, "session %s (task %s) is working on %v", r.RunID, r.TaskID, r.Scope)
		}
	}
	for _, t := range in.Awaiting {
		if t.ID != in.Task.ID && glob.AnyOverlap(scope, t.Scope) {
			return deferFor(ReasonPendingLanding, "task %s has finished work on %v that has not landed", t.ID, t.Scope)
		}
	}
	for _, t := range in.Tasks {
		if t.ID != in.Task.ID && t.State == agwp.TaskInProgress && glob.AnyOverlap(scope, t.Scope) {
			return deferFor(ReasonTaskInProgress, "task %s is in progress on %v", t.ID, t.Scope)
		}
	}
	set, err := glob.CompileAll(scope)
	if err == nil {
		for _, p := range in.OpenProposals {
			if p.TaskID == in.Task.ID {
				continue
			}
			for _, path := range in.ProposalPaths[p.ID] {
				if set.Match(path) {
					return deferFor(ReasonOpenProposal, "proposal %s already changes %s", p.ID, path)
				}
			}
		}
		for _, o := range in.Overlaps {
			if hit := predictedHit(o, scope, set); hit != "" {
				return deferFor(ReasonPredicted, "%s: %s", o.Kind, hit)
			}
		}
	}
	return start()
}

// predictedHit returns the part of a predicted overlap that touches the scope.
func predictedHit(o agwp.Overlap, scope []string, set glob.Set) string {
	switch o.Kind {
	case "leases_overlap", "task_scopes_overlap":
		// paths are scope globs ("a" / "a ~ b"); ours must overlap one of them
		for _, p := range o.Paths {
			for _, part := range strings.Split(p, " ~ ") {
				if glob.AnyOverlap(scope, []string{part}) {
					return part
				}
			}
		}
	default:
		// proposal related kinds list changed file paths
		for _, p := range o.Paths {
			if set.Match(p) {
				return p
			}
		}
	}
	return ""
}
