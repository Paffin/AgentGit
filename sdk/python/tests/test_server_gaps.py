"""Executable notes on the server contract the SDK depends on.

``fixed`` tests pin server behaviour that used to differ from docs/protocol and that the SDK now relies on
(the SDK has no workaround for any of it). ``remaining`` tests pin what still differs or is surprising; a
failure there is good news - the server changed: update the guide ("What the SDKs work around") and the
SDK workaround, if any.
"""

from __future__ import annotations

import json

import httpx
import pytest

from agentgit import (
    AgentGitClient,
    AlreadyExistsError,
    CallOptions,
    IdempotencyKeyReusedError,
    blob_object,
    generate_agent_key,
    put_file,
)
from tests.harness import REPO_ROOT, RunningServer, start_server

REPO = "gaps/one"
KEY = CallOptions(idempotency_key="same-key")


@pytest.fixture(scope="module")
def admin(server: RunningServer) -> AgentGitClient:
    c = server.admin(retry=False)
    c.repos.create(name=REPO)
    return c


def auth(server: RunningServer) -> dict[str, str]:
    return {"authorization": f"Bearer {server.bootstrap_token}"}


# --------------------------------------------------------------------------- fixed on the server


def test_seven_creating_operations_honour_idempotency_key(
    server: RunningServer, admin: AgentGitClient
) -> None:
    a = admin.repos.create(name="gaps/keyed", options=KEY)
    b = admin.repos.create(name="gaps/keyed", options=KEY)
    assert b.created_at_ms == a.created_at_ms

    t1 = admin.tasks.create(repo=REPO, title="once", options=KEY)
    t2 = admin.tasks.create(repo=REPO, title="once", options=KEY)
    assert t2.id == t1.id
    assert [t.title for t in admin.tasks.list(repo=REPO).tasks].count("once") == 1
    # the same key with different content is refused, not replayed
    with pytest.raises(IdempotencyKeyReusedError):
        admin.tasks.create(repo=REPO, title="another", options=KEY)

    with server.actor("agent:gap-bot", REPO, "writer", retry=False) as bot:
        bot.agents.register(repo=REPO, name="gap-bot", public_key=generate_agent_key().public_key_hex)
        r1 = bot.agents.start_run(repo=REPO, agent="gap-bot", options=KEY)
        r2 = bot.agents.start_run(repo=REPO, agent="gap-bot", options=KEY)
        assert r2.run_id == r1.run_id
        assert len(bot.agents.list_runs(repo=REPO, agent="gap-bot").runs) == 1

        l1 = bot.leases.acquire(repo=REPO, scope="gap/**", exclusive=True, ttl_ms=60_000, options=KEY)
        l2 = bot.leases.acquire(repo=REPO, scope="gap/**", exclusive=True, ttl_ms=60_000, options=KEY)
        assert l2.lease.id == l1.lease.id
        assert l2.lease.fencing_token == l1.lease.fencing_token


def test_capabilities_takes_list_valued_parameters_as_repeated_keys(server: RunningServer) -> None:
    def get(query: str) -> httpx.Response:
        return httpx.get(f"{server.base_url}/v1/capabilities?{query}")

    everything = get("").json()["features"]
    assert everything
    # the requested features are intersected with the server's
    both = get(f"supported_versions=agwp.v1&features={everything[0]}&features=nope")
    assert both.status_code == 200
    assert both.json()["protocol_version"] == "agwp.v1"
    assert both.json()["features"] == [everything[0]]
    # a list of versions: the server picks the one it speaks
    assert get("supported_versions=agwp.v0&supported_versions=agwp.v1").status_code == 200
    assert get("supported_versions=agwp.v0").status_code == 412
    # "a,b" is one (unknown) feature, not two
    assert get(f"features={everything[0]},nope").json()["features"] == []


def test_replay_beyond_the_end_returns_nothing_and_the_real_latest_seq(admin: AgentGitClient) -> None:
    latest = admin.events.replay(repo=REPO, after_seq=0, limit=1).latest_seq
    assert latest > 0
    for after in (18446744073709551615, 2**53 - 1):
        beyond = admin.events.replay(repo=REPO, after_seq=after, limit=1)
        assert beyond.events == []
        assert beyond.latest_seq == latest


def test_queue_entries_finish_as_done_with_outcome_integrated(admin: AgentGitClient) -> None:
    admin.workspaces.create(repo=REPO, name="q", stream="main")
    change = admin.workspaces.modify(repo=REPO, name="q", changes=[put_file("q.txt", "q")])
    p = admin.proposals.create(repo=REPO, stream="main", head=change.head, title="q")
    admin.queue.enqueue(repo=REPO, proposal=p.id)
    ran = admin.queue.run(repo=REPO, stream="main")
    assert (ran.processed[0].state, ran.processed[0].outcome) == ("done", "integrated")


def test_tree_and_directory_listings_carry_real_sizes(admin: AgentGitClient) -> None:
    entry = next(e for e in admin.tree.get(repo=REPO, rev="main").entries if e.path == "q.txt")
    assert entry.size == 1
    assert admin.tree.list(repo=REPO, rev="main").entries[0].size == 1
    resolved = admin.tree.resolve(repo=REPO, rev="main", path="q.txt")
    assert resolved.entry is not None
    assert resolved.entry.size == 1


def test_a_wrong_method_or_an_unknown_route_answers_in_the_envelope(server: RunningServer) -> None:
    wrong = httpx.delete(
        f"{server.base_url}/v1/repos/{REPO}", headers={**auth(server), "x-request-id": "gap-405"}
    )
    assert wrong.status_code == 405
    assert wrong.headers["allow"]
    assert wrong.json()["error"]["code"] == "invalid_argument"
    assert wrong.json()["error"]["request_id"] == "gap-405"
    unknown = httpx.get(f"{server.base_url}/v1/nope", headers={**auth(server), "x-request-id": "gap-404"})
    assert unknown.status_code == 404
    assert unknown.json()["error"]["code"] == "not_found"
    assert unknown.json()["error"]["request_id"] == "gap-404"


def test_file_type_is_regular_on_reads_and_writes_and_file_is_still_accepted(admin: AgentGitClient) -> None:
    cs = admin.history.changeset(repo=REPO, rev="main", include_ops=True)
    assert cs.ops[0].file_type == "regular"
    for path, file_type in (("t-regular", "regular"), ("t-alias", "file")):
        res = admin.workspaces.modify(
            repo=REPO,
            name="q",
            changes=[{"path": path, "op": "put", "content": "eg==", "file_type": file_type}],
        )
        assert len(res.head) == 64
    # the `kind` of an entry is a different vocabulary: file | symlink | dir
    assert admin.tree.resolve(repo=REPO, rev="@q", path="t-regular").kind == "file"


# --------------------------------------------------------------------------- remaining


def test_stream_create_and_credentials_mint_ignore_idempotency_key_by_design(
    admin: AgentGitClient,
) -> None:
    # a repeated stream.create answers already_exists ...
    admin.streams.create(repo=REPO, name="dev", options=KEY)
    with pytest.raises(AlreadyExistsError):
        admin.streams.create(repo=REPO, name="dev", options=KEY)
    # ... and a credential's secret can be shown once, so a repeated mint is a second credential
    c1 = admin.credentials.mint(subject="user:x", ttl_secs=60, options=KEY)
    c2 = admin.credentials.mint(subject="user:x", ttl_secs=60, options=KEY)
    assert c1.credential.id != c2.credential.id


def test_objects_put_answers_json_although_openapi_lists_octet_stream(server: RunningServer) -> None:
    spec = json.loads((REPO_ROOT / "docs/protocol/openapi.json").read_text())
    put = next(
        op for item in spec["paths"].values() for op in item.values() if op["operationId"] == "objects.put"
    )
    assert list(put["responses"]["200"]["content"]) == ["application/octet-stream"]

    blob = blob_object("gap\n")
    res = httpx.put(
        f"{server.base_url}/v1/repos/{REPO}/objects/{blob.id}",
        headers={**auth(server), "content-type": "application/octet-stream"},
        content=blob.envelope,
    )
    assert res.status_code == 200
    assert res.headers["content-type"].startswith("application/json")
    assert res.json()["object_id"] == blob.id


def test_retry_after_is_whole_seconds_and_at_least_one_while_retry_after_ms_can_be_shorter() -> None:
    limited = start_server(config="[limits]\nrequests_per_second = 20\nburst = 2\n")
    try:
        seen: tuple[str | None, int] | None = None
        for _ in range(40):
            res = httpx.get(f"{limited.base_url}/v1/whoami", headers=auth(limited))
            if res.status_code == 429:
                seen = (res.headers.get("retry-after"), res.json()["error"]["retry_after_ms"])
                break
        assert seen is not None
        header, ms = seen
        assert header == "1"
        assert 0 < ms < 1000  # the SDK waits for this, not for the rounded header
    finally:
        limited.stop()
