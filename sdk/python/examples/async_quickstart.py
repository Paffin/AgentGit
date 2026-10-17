"""The quick start with the asyncio client.

AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... python examples/async_quickstart.py
"""

# region async
import asyncio
import os
import time

from agentgit import AsyncAgentGitClient, put_file


async def main() -> str:
    async with AsyncAgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"]) as client:
        repo = f"demo/async-{int(time.time() * 1000)}"
        await client.repos.create(name=repo)
        await client.workspaces.create(repo=repo, name="hello", stream="main")
        change = await client.workspaces.modify(repo=repo, name="hello", changes=[put_file("a.txt", "a\n")])
        proposal = await client.proposals.create(repo=repo, stream="main", head=change.head, title="A")
        result = await client.integration.integrate(repo=repo, proposal=proposal.id)

        # Listings are async iterators; so is the event feed.
        async for changeset in client.paginate("history.log", repo=repo, rev="main"):
            print(changeset.id[:12], changeset.message)
        return result.outcome.status


status = asyncio.run(main())
print(status)  # integrated
# endregion async

# What the test checks.
async_result = {"status": status}
