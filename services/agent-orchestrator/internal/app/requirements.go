package app

import (
	"sort"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// OrchestratorOps are the operations (server capabilities) the orchestrator's
// own credential needs. Everything it hands to agents must be contained in it,
// because the server only passes on capabilities the minting credential holds.
// The list is derived from the configuration.
//
// The orchestrator needs no administrator: the role bound to service:orchestrator
// is `delegator`, which may mint credentials for agent:* subjects only
// (credentials.delegate) and operate their tasks and runs (agent.operate). The
// credential's scope is what limits the service further. Notably absent:
// integrate.run, proposal.review, policy.write, stream.cas, gc.run, roles.manage,
// credentials.mint.
func OrchestratorOps(cfg *config.Config) []string {
	set := map[string]bool{
		"repo.read": true, "stream.read": true, "task.read": true, "lease.read": true, "agent.read": true, "agent.register": true,
		"agent.run": true, "proposal.read": true, "integrate.prepare": true, "predict": true, "events.read": true, "tree.read": true,
		"history.read": true, "jobs.read": true, "queue.read": true,
		// mint, list and revoke the credentials of agents (and nobody else's), bound to their runs
		"credentials.delegate": true,
		// finish the task of a run that ended, and take over the runs of a session that was lost
		"agent.operate": true, "task.write": true,
	}
	for _, r := range cfg.Repos {
		if config.On(r.ExecSched.Enqueue) && config.On(r.ExecSched.Enabled) {
			set["integrate.validate"] = true
		}
	}
	for _, e := range cfg.Execs {
		for _, op := range e.Ops {
			set[op] = true
		}
	}
	return sorted(set)
}

// ReviewerOps are the operations the reviewer identity needs.
func ReviewerOps() []string {
	return []string{"repo.read", "stream.read", "proposal.read", "proposal.review", "tree.read", "history.read"}
}

// AdvisorOps are the operations the advisor identity needs.
func AdvisorOps() []string {
	return []string{"repo.read", "proposal.read", "proposal.review", "tree.read"}
}

func sorted(set map[string]bool) []string {
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
