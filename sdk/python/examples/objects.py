"""Build a changeset on the client, upload only the objects the server lacks, and propose it.

AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... python examples/objects.py
"""

import os
import time

from agentgit import AgentGitClient

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
repo = f"demo/objects-{int(time.time() * 1000)}"
client.repos.create(name=repo)

# region objects
from agentgit import PutOp, build_changeset

info = client.repos.describe(repo=repo)  # the namespace is part of every changeset hash
me = client.session.whoami()
parent = client.streams.head(repo=repo, stream="main").head  # "" while the stream is empty

built = build_changeset(
    namespace=info.namespace,
    parents=[parent] if parent else [],
    author=me.identity,
    timestamp_ms=int(time.time() * 1000),
    message="add docs",
    ops=[
        PutOp("docs/guide.md", content="# guide\n"),
        PutOp("docs/copy.md", content="# guide\n"),  # same bytes: stored once
    ],
)

# Blobs first, then the changeset that names them. Objects the server already has are skipped.
upload = client.objects.upload(repo=repo, objects=built.objects())
print(len(upload.stored), len(upload.already_present))  # 2 0

# The server derives the same id from the bytes, so the id can be proposed directly.
proposal = client.proposals.create(repo=repo, stream="main", head=built.id, title="Add docs")
landed = client.integration.integrate(repo=repo, proposal=proposal.id)
print(landed.outcome.status)  # integrated
# endregion objects

# What the test checks.
objects_result = {
    "repo": repo,
    "id": built.id,
    "new_head": landed.outcome.new_head,
    "stored": len(upload.stored),
}
