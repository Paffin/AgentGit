"""Preflight a Git bundle; set AGENTGIT_PUBLISH=1 only after reviewing the report."""

# region migration
import os
from pathlib import Path

from agentgit import AgentGitClient, CallOptions

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
repo = os.environ["AGENTGIT_REPO"]  # a new owner/name
plan = client.migrations.prepare(
    name=repo,
    data=Path(os.environ.get("AGENTGIT_BUNDLE", "repository.bundle")).read_bytes(),
    options=CallOptions(timeout_ms=300_000),
)
print(plan.id, plan.report)
# Review the report before opting into publication. Content omissions remain blocked.
if os.environ.get("AGENTGIT_PUBLISH") == "1":
    client.migrations.commit(id=plan.id, accept_lossy=False)
# endregion migration

migration_result = client.migrations.status(id=plan.id)
client.close()
