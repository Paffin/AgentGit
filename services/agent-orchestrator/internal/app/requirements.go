package app

import (
	"sort"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// OrchestratorOps are the operations (server capabilities) the orchestrator's
// own credential needs. Everything it hands to agents must be contained in it,
// because the server refuses to mint a credential wider than the one used to
// mint it. The list is derived from the configuration.
//
// The credential is meant to be minted with exactly these operations: the role
// bound to service:orchestrator may be broader (the server only lets
// administrators mint credentials for other identities), but the credential's
// scope is what actually limits the service. Notably absent: integrate.run,
// proposal.review, policy.write, stream.cas, gc.run, roles.manage.
func OrchestratorOps(cfg *config.Config) []string {
	set := map[string]bool{
		"repo.read": true, "stream.read": true, "task.read": true, "lease.read": true, "agent.read": true, "agent.register": true,
		"agent.run": true, "proposal.read": true, "integrate.prepare": true, "predict": true, "events.read": true, "tree.read": true,
		"history.read": true, "jobs.read": true, "queue.read": true, "credentials.mint": true, "credentials.revoke": true,
		// the narrow credential used to finish a task as its owner
		"task.write": true,
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
