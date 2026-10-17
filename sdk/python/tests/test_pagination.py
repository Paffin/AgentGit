from __future__ import annotations

import asyncio

import httpx
import pytest

from agentgit import AgentGitClient, InvalidArgumentError, StaleCandidateError, put_file
from tests.faults import FaultyTransport
from tests.harness import RunningServer


@pytest.fixture(scope="module")
def net() -> FaultyTransport:
    return FaultyTransport()


@pytest.fixture(scope="module")
def admin(server: RunningServer, net: FaultyTransport) -> AgentGitClient:
    return server.admin(http_client=httpx.Client(transport=net))


def test_follows_next_page_token_across_repository_listings(
    admin: AgentGitClient, net: FaultyTransport
) -> None:
    for i in range(23):
        admin.repos.create(name=f"pages/r{i:02d}")
    net.reset()
    repos = list(admin.paginate("repo.list", prefix="pages/", page_size=10))
    assert [r.name for r in repos] == [f"pages/r{i:02d}" for i in range(23)]
    calls = net.to("/v1/repos?")
    assert len(calls) == 3  # 23 items at 10 per page, each carrying the previous page's token
    assert httpx.URL(calls[0].url).params["page_token"] == ""
    assert httpx.URL(calls[1].url).params["page_token"] != ""


def test_pages_yields_whole_pages_and_max_items_stops_early(
    admin: AgentGitClient, net: FaultyTransport
) -> None:
    pages = list(admin.pages("repo.list", prefix="pages/", page_size=10))
    assert [len(p.repos) for p in pages] == [10, 10, 3]
    assert [p.next_page_token == "" for p in pages] == [False, False, True]
    net.reset()
    assert len(list(admin.paginate("repo.list", prefix="pages/", page_size=10, max_items=4))) == 4
    assert len(net.to("/v1/repos?")) == 1  # the second page was never requested


def test_paginates_history_trees_proposals_and_the_candidate_listing(
    server: RunningServer, admin: AgentGitClient
) -> None:
    repo = "pages/work"
    admin.repos.create(name=repo)
    alice = server.actor("user:alice", repo, "writer")
    carol = server.actor("user:carol", repo, "integrator")
    alice.workspaces.create(repo=repo, name="main-ws", stream="main")
    head = ""
    for i in range(12):
        head = alice.workspaces.modify(
            repo=repo, name="main-ws", message=f"change {i}", changes=[put_file(f"f{i:02d}.txt", f"v{i}\n")]
        ).head
    p = alice.proposals.create(repo=repo, stream="main", head=head, title="twelve commits")
    assert carol.integration.integrate(repo=repo, proposal=p.id).outcome.status == "integrated"

    log = list(alice.paginate("history.log", repo=repo, rev="main", page_size=5))
    assert [c.message for c in log] == [f"change {11 - i}" for i in range(12)]
    entries = list(alice.paginate("tree.get", repo=repo, rev="main", page_size=5))
    assert len(entries) == 12
    assert [e.path for e in entries] == sorted(e.path for e in entries)
    assert len(list(alice.paginate("tree.list", repo=repo, rev="main", page_size=5))) == 12

    for i in range(7):
        alice.workspaces.create(repo=repo, name=f"p{i}", stream="main")
        w = alice.workspaces.modify(repo=repo, name=f"p{i}", changes=[put_file(f"extra{i}.txt", "x")])
        alice.proposals.create(repo=repo, stream="main", head=w.head, title=f"extra {i}")
    open_ = list(alice.paginate("proposal.list", repo=repo, state="open", author="user:alice", page_size=3))
    assert len(open_) == 7
    assert all(x.state == "open" for x in open_)
    assert len(list(alice.paginate("proposal.list", repo=repo, page_size=3))) == 8

    # The worker view of a candidate tree is a paginated manifest too (12 files plus the proposal's own).
    candidate = list(alice.paginate("jobs.candidate", repo=repo, proposal=open_[0].id, page_size=5))
    assert len(candidate) == 13
    with pytest.raises(StaleCandidateError) as stale:
        alice.jobs.candidate(repo=repo, proposal=open_[0].id, expected_tree_id="ab" * 32)
    assert stale.value.status == 409


def test_ancestors_paginates_changeset_ids(server: RunningServer, admin: AgentGitClient) -> None:
    repo = "pages/work"
    head = admin.streams.head(repo=repo, stream="main").head
    ids = list(admin.paginate("history.ancestors", repo=repo, want=[head], have=[], page_size=5))
    assert len(ids) >= 12
    assert len(set(ids)) == len(ids)


def test_an_unknown_page_token_is_an_error_from_the_server(admin: AgentGitClient) -> None:
    with pytest.raises(InvalidArgumentError):
        admin.repos.list(page_token="garbage")


def test_a_server_that_keeps_returning_the_same_token_cannot_loop_forever() -> None:
    calls = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return httpx.Response(200, json={"repos": [{"name": f"x/{calls}"}], "next_page_token": "same"})

    with AgentGitClient(
        "http://srv.test", "t", http_client=httpx.Client(transport=httpx.MockTransport(handler))
    ) as c:
        assert (
            len(list(c.paginate("repo.list"))) == 2
        )  # first page, then one that repeats the token, then stop
    assert calls == 2
    with AgentGitClient("http://srv.test", "t") as c, pytest.raises(ValueError, match="not a paginated"):
        list(c.pages("repo.create"))


def test_the_async_client_paginates_too(server: RunningServer) -> None:
    async def scenario() -> None:
        async with server.async_admin() as admin:
            names = [r.name async for r in admin.paginate("repo.list", prefix="pages/r", page_size=10)]
            assert names == [f"pages/r{i:02d}" for i in range(23)]
            assert (
                len(
                    [r async for r in admin.paginate("repo.list", prefix="pages/", page_size=10, max_items=4)]
                )
                == 4
            )

    asyncio.run(scenario())
