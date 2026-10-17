"""A repository, a change made without a checkout, a proposal, an integration, and the history.

AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... python examples/quickstart.py
"""

# region quickstart
import os
import time

from agentgit import AgentGitClient, put_file

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
client.negotiate(client_name="quickstart")  # checks that the server speaks agwp.v1

repo = f"demo/hello-{int(time.time() * 1000)}"
client.repos.create(name=repo)

# Change files on the server, in a virtual workspace: no checkout needed.
client.workspaces.create(repo=repo, name="hello", stream="main")
change = client.workspaces.modify(
    repo=repo,
    name="hello",
    message="add a greeting",
    changes=[put_file("README.md", "# hello\n"), put_file("src/main.rs", "fn main() {}\n")],
)

# Propose it, and land it on the stream "main".
proposal = client.proposals.create(repo=repo, stream="main", head=change.head, title="Add a greeting")
result = client.integration.integrate(repo=repo, proposal=proposal.id)
print(result.outcome.status)  # integrated

# Read it back.
readme = client.tree.file(repo=repo, path="README.md", rev="main")
print(readme.decode())  # "# hello"
for changeset in client.paginate("history.log", repo=repo, rev="main"):
    print(changeset.id[:12], changeset.message)
# endregion quickstart

# What the test checks.
quickstart_result = {"repo": repo, "status": result.outcome.status, "readme": readme.decode()}
