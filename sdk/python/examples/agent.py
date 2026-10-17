"""An agent run end to end: its own key, a run, work in a virtual workspace, a proposal,
signed provenance, and a human who approves and integrates.

    AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_ADMIN_TOKEN=... python examples/agent.py
"""

import json
import os
import time

from agentgit import AgentGitClient, generate_agent_key, put_file, sign_provenance

url = os.environ["AGENTGIT_URL"]
admin = AgentGitClient(url, os.environ["AGENTGIT_ADMIN_TOKEN"])

# region agent-setup
# An operator creates the repository, requires signed provenance from agents, and gives the agent a
# short-lived credential that is limited to this repository.
repo = f"demo/agent-{int(time.time() * 1000)}"
admin.repos.create(name=repo)
admin.admin.policy_set(
    repo=repo,
    stream="main",
    document=json.dumps({"require_provenance_for_agents": True, "approvals": {"min_for_agents": 1}}),
)
admin.roles.grant(subject="agent:docs-bot", repo=repo, role="writer")
admin.roles.grant(subject="user:reviewer", repo=repo, role="reviewer")
admin.roles.grant(subject="user:maintainer", repo=repo, role="integrator")
minted = admin.credentials.mint(subject="agent:docs-bot", ttl_secs=900, scope={"repos": [repo]})
# endregion agent-setup

# region agent
bot = AgentGitClient(url, minted.token)

# The agent registers the public half of its own key, then starts a run.
key = generate_agent_key()
bot.agents.register(repo=repo, name="docs-bot", public_key=key.public_key_hex)
run = bot.agents.start_run(repo=repo, agent="docs-bot")
print(run.identity)  # agent:docs-bot#run_...

# Everything the agent does from here on is attributed to this run.
work = bot.with_run(run.run_id)
work.workspaces.create(repo=repo, name="docs", stream="main")
change = work.workspaces.modify(
    repo=repo,
    name="docs",
    message="document the build",
    changes=[put_file("docs/build.md", "# Building\n\nRun `make`.\n")],
)
proposal = work.proposals.create(repo=repo, stream="main", head=change.head, title="Document the build")

# Signed provenance: who made this change, with what, and why. The server verifies the signature
# against the key registered for the agent.
record = {
    "changeset": change.head,
    "agent": run.identity,
    "run_id": run.run_id,
    "model": "example-model",
    "rationale": "the build steps were undocumented",
    "created_at_ms": int(time.time() * 1000),
}
entry = work.provenance.submit(repo=repo, record=record, signature=sign_provenance(record, key.private_key))
print(entry.verified)  # True

work.agents.complete(repo=repo, run_id=run.run_id, status="completed", summary="documented the build")
# endregion agent


def human(subject: str) -> AgentGitClient:
    return AgentGitClient(url, admin.credentials.mint(subject=subject, ttl_secs=900).token)


# region agent-review
# Humans decide. The policy above holds the change until someone approves it, and only an
# integrator can land it.
reviewer = human("user:reviewer")
maintainer = human("user:maintainer")
held = maintainer.integration.integrate(repo=repo, proposal=proposal.id)
print(held.outcome.status, [b.code for b in held.outcome.blockers])  # blocked ['insufficient_approvals']

reviewer.proposals.review(repo=repo, id=proposal.id, decision="approve", body="looks right")
landed = maintainer.integration.integrate(repo=repo, proposal=proposal.id)
print(landed.outcome.status)  # integrated
# endregion agent-review

# What the test checks.
agent_result = {
    "repo": repo,
    "run_identity": run.identity,
    "verified": entry.verified,
    "held": [b.code for b in held.outcome.blockers],
    "landed": landed.outcome.status,
}
