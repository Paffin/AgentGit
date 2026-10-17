"""Executable notes on where ag-server 0.1.0 differs from what docs/protocol and agwp.proto promise.

The SDK is written around these. A failure here is good news: the server changed - update the
"Things the server does today that show through" section of the README and the SDK workarounds.
"""

from __future__ import annotations

import httpx
import pytest

from agentgit import AgentGitClient, AlreadyExistsError, CallOptions, InvalidArgumentError, put_file
from tests.harness import RunningServer

REPO = "gaps/one"


@pytest.fixture(scope="module")
def admin(server: RunningServer) -> AgentGitClient:
    c = server.admin(retry=False)
    c.repos.create(name=REPO)
    return c


def test_only_four_operations_honour_idempotency_key(admin: AgentGitClient) -> None:
    key = CallOptions(idempotency_key="same-key")
    # honoured: the second call returns the recorded result
    a = admin.repos.create(name="gaps/keyed", options=key)
    b = admin.repos.create(name="gaps/keyed", options=key)
    assert b.created_at_ms == a.created_at_ms
    # ignored: the fields exist in the messages and the OpenAPI document, but nothing is recorded
    admin.streams.create(repo=REPO, name="dev", options=key)
    with pytest.raises(AlreadyExistsError):
        admin.streams.create(repo=REPO, name="dev", options=key)
    t1 = admin.tasks.create(repo=REPO, title="dup", options=key)
    t2 = admin.tasks.create(repo=REPO, title="dup", options=key)
    assert t1.id != t2.id
    c1 = admin.credentials.mint(subject="user:x", ttl_secs=60, options=key)
    c2 = admin.credentials.mint(subject="user:x", ttl_secs=60, options=key)
    assert c1.credential.id != c2.credential.id


def test_capabilities_cannot_take_the_array_parameters_its_openapi_entry_lists(server: RunningServer) -> None:
    for query in ("supported_versions=agwp.v1", "features=a&features=b", "features=a,b"):
        assert httpx.get(f"{server.base_url}/v1/capabilities?{query}").status_code == 400, query


def test_replay_treats_after_seq_u64_max_as_from_the_start(admin: AgentGitClient) -> None:
    assert admin.events.replay(repo=REPO, after_seq=18446744073709551615, limit=1).events[0].seq == 1
    big = admin.events.replay(repo=REPO, after_seq=2**53 - 1, limit=1)
    assert big.events == []
    assert big.latest_seq > 0


def test_queue_entries_finish_as_done_with_an_outcome(admin: AgentGitClient) -> None:
    admin.workspaces.create(repo=REPO, name="q", stream="main")
    change = admin.workspaces.modify(repo=REPO, name="q", changes=[put_file("q.txt", "q")])
    p = admin.proposals.create(repo=REPO, stream="main", head=change.head, title="q")
    admin.queue.enqueue(repo=REPO, proposal=p.id)
    ran = admin.queue.run(repo=REPO, stream="main")
    assert (ran.processed[0].state, ran.processed[0].outcome) == ("done", "integrated")


def test_tree_listings_report_size_zero_where_resolve_knows_the_size(admin: AgentGitClient) -> None:
    entry = next(e for e in admin.tree.get(repo=REPO, rev="main").entries if e.path == "q.txt")
    resolved = admin.tree.resolve(repo=REPO, rev="main", path="q.txt")
    assert entry.size == 0
    assert resolved.entry is not None
    assert resolved.entry.size == 1
    assert admin.tree.list(repo=REPO, rev="main").entries[0].size == 0


def test_a_405_has_no_json_body_and_an_unknown_route_has_no_request_id(server: RunningServer) -> None:
    headers = {"authorization": f"Bearer {server.bootstrap_token}"}
    res = httpx.delete(f"{server.base_url}/v1/repos/{REPO}", headers=headers)
    assert (res.status_code, res.text) == (405, "")
    unknown = httpx.get(f"{server.base_url}/v1/nope", headers=headers)
    assert unknown.status_code == 404
    assert unknown.json()["error"]["request_id"] == ""


def test_file_types_are_file_on_reads_but_regular_on_writes(admin: AgentGitClient) -> None:
    cs = admin.history.changeset(repo=REPO, rev="main", include_ops=True)
    assert cs.ops[0].file_type == "file"
    with pytest.raises(InvalidArgumentError):
        admin.workspaces.modify(
            repo=REPO, name="q", changes=[{"path": "z", "op": "put", "content": "eg==", "file_type": "file"}]
        )
