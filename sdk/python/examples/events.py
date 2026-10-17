"""Follow a repository's event feed: replay the past, then stay live. The connection resumes after the
last event it delivered if it drops.

    AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... python examples/events.py
"""

import os
import time

from agentgit import AgentGitClient, put_file

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
repo = f"demo/events-{int(time.time() * 1000)}"
client.repos.create(name=repo)

# region events
import threading

seen: list[str] = []


# Something happens in the repository while we listen ...
def work() -> None:
    client.workspaces.create(repo=repo, name="w", stream="main")
    change = client.workspaces.modify(repo=repo, name="w", changes=[put_file("a.txt", "a\n")])
    proposal = client.proposals.create(repo=repo, stream="main", head=change.head, title="A")
    client.integration.integrate(repo=repo, proposal=proposal.id)


worker = threading.Thread(target=work)
worker.start()

# after_seq=0 replays the whole history first; leave it out to start from "now".
# topics are prefixes: "proposal" matches proposal.create, "stream" matches stream.integrate.
feed = client.subscribe_events(repo=repo, after_seq=0, topics=["proposal", "stream"])

# ... and each event arrives exactly once, in order, with a strictly increasing seq.
for event in feed:
    print(event.seq, event.topic, event.payload["target"])
    seen.append(event.topic)
    if event.topic == "stream.integrate":
        break  # closes the connection
worker.join()
# endregion events

# What the test checks.
events_result = {"seen": seen}
