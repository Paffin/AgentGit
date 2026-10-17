"""A complete change life cycle against a real server: a writer edits a virtual workspace, opens a
proposal, a reviewer approves, an integrator integrates, and everything is read back."""

from __future__ import annotations

import asyncio

import pytest

from agentgit import (
    AgentGitClient,
    CallOptions,
    NotFoundError,
    ObjectCorruptError,
    PermissionDeniedError,
    PutOp,
    blob_object,
    build_changeset,
    delete_file,
    put_file,
)
from tests.harness import RunningServer

REPO = "acme/web"


@pytest.fixture(scope="module")
def team(server: RunningServer) -> dict[str, AgentGitClient]:
    admin = server.admin()
    admin.repos.create(name=REPO, description="demo")
    admin.admin.policy_set(repo=REPO, stream="main", document='{"approvals":{"min":1}}')
    return {
        "admin": admin,
        "alice": server.actor("user:alice", REPO, "writer"),
        "bob": server.actor("user:bob", REPO, "reviewer"),
        "carol": server.actor("user:carol", REPO, "integrator"),
    }


class State:
    proposal_id = ""
    head = ""


def test_a_writer_changes_files_in_a_virtual_workspace_and_opens_a_proposal(
    team: dict[str, AgentGitClient],
) -> None:
    alice = team["alice"]
    ws = alice.workspaces.create(repo=REPO, name="ws-1", stream="main")
    assert ws.owner == "user:alice"
    changed = alice.workspaces.modify(
        repo=REPO,
        name="ws-1",
        message="initial import",
        changes=[
            put_file("README.md", "# acme\n"),
            put_file("src/lib.rs", "fn main() {}\n"),
            put_file("bin/run.sh", "#!/bin/sh\n", executable=True),
        ],
    )
    State.head = changed.head
    assert len(State.head) == 64
    proposal = alice.proposals.create(
        repo=REPO, stream="main", head=State.head, title="Initial import", description="first files"
    )
    State.proposal_id = proposal.id
    assert (proposal.state, proposal.author, proposal.head) == ("open", "user:alice", State.head)


def test_separation_of_duties(team: dict[str, AgentGitClient]) -> None:
    with pytest.raises(PermissionDeniedError):
        team["alice"].proposals.review(repo=REPO, id=State.proposal_id, decision="approve")
    with pytest.raises(PermissionDeniedError):
        team["alice"].integration.integrate(repo=REPO, proposal=State.proposal_id)


def test_an_integration_without_approval_is_blocked_with_a_precise_reason(
    team: dict[str, AgentGitClient],
) -> None:
    carol, alice = team["carol"], team["alice"]
    result = carol.integration.integrate(repo=REPO, proposal=State.proposal_id)
    assert result.outcome.status == "blocked"
    assert result.outcome.blockers[0].code == "insufficient_approvals"
    assert alice.streams.head(repo=REPO, stream="main").head == ""
    prep = carol.integration.prepare(repo=REPO, proposal=State.proposal_id)
    assert prep.strategy == "fast_forward"
    assert sorted(prep.changed_files) == ["README.md", "bin/run.sh", "src/lib.rs"]
    assert len(prep.blockers) == 1


def test_a_reviewer_approves_and_the_integrator_lands_it_exactly_once(
    team: dict[str, AgentGitClient],
) -> None:
    alice, bob, carol = team["alice"], team["bob"], team["carol"]
    review = bob.proposals.review(repo=REPO, id=State.proposal_id, decision="approve", body="lgtm")
    assert (review.decision, review.reviewer, review.head) == ("approve", "user:bob", State.head)
    assert len(alice.proposals.get(repo=REPO, id=State.proposal_id).reviews) == 1

    done = carol.integration.integrate(
        repo=REPO, proposal=State.proposal_id, options=CallOptions(idempotency_key="integrate-once")
    )
    assert done.outcome.status == "integrated"
    assert (
        done.outcome.new_head,
        done.outcome.strategy,
        done.outcome.version,
        done.outcome.changed_files,
    ) == (
        State.head,
        "fast_forward",
        1,
        3,
    )
    # The same key returns the recorded result instead of integrating again ...
    again = carol.integration.integrate(
        repo=REPO, proposal=State.proposal_id, options=CallOptions(idempotency_key="integrate-once")
    )
    assert again.operation_id == done.operation_id
    # ... and the outcome can be asked for after a lost response.
    op = carol.operations.get(repo=REPO, idempotency_key="integrate-once")
    assert op.integration is not None
    assert op.integration.outcome.status == "integrated"
    assert alice.proposals.get(repo=REPO, id=State.proposal_id).proposal.state == "integrated"


def test_reads_everything_back(team: dict[str, AgentGitClient]) -> None:
    alice, admin = team["alice"], team["admin"]
    assert (
        alice.streams.head(repo=REPO, stream="main").head,
        alice.streams.head(repo=REPO, stream="main").version,
    ) == (
        State.head,
        1,
    )
    lib = alice.tree.file(repo=REPO, path="src/lib.rs", rev="main")
    assert isinstance(lib, bytes)
    assert lib == b"fn main() {}\n"
    listing = alice.tree.list(repo=REPO, rev="main")
    assert sorted(e.name for e in listing.entries) == ["README.md", "bin", "src"]
    resolved = alice.tree.resolve(repo=REPO, rev="main", path="bin/run.sh")
    assert resolved.kind == "file"
    assert resolved.entry is not None
    assert resolved.entry.executable
    tree = alice.tree.get(repo=REPO, rev="main")
    assert tree.total_entries == 3
    assert [e.path for e in tree.entries] == ["README.md", "bin/run.sh", "src/lib.rs"]
    assert [c.message for c in alice.history.log(repo=REPO, rev="main").changesets] == ["initial import"]
    cs = alice.history.changeset(repo=REPO, rev="main", include_ops=True)
    assert [f"{o.op}:{o.path}" for o in cs.ops] == ["put:README.md", "put:bin/run.sh", "put:src/lib.rs"]
    assert alice.history.resolve(repo=REPO, rev="main").changeset_id == State.head
    diff = alice.tree.diff(repo=REPO, to="main")
    assert [f.status for f in diff.files] == ["added", "added", "added"]
    assert "+# acme" in next(f for f in diff.files if f.path == "README.md").patch
    assert alice.streams.reflog(repo=REPO, stream="main").entries[0].new_head == State.head
    topics = [e.topic for e in alice.events.replay(repo=REPO, after_seq=0, limit=100).events]
    assert {"workspace.modify", "proposal.create", "stream.integrate"} <= set(topics)
    assert admin.admin.audit_verify(repo=REPO).ok


def test_a_later_change_edits_and_deletes_files(team: dict[str, AgentGitClient]) -> None:
    alice, bob, carol = team["alice"], team["bob"], team["carol"]
    alice.workspaces.create(repo=REPO, name="ws-2", stream="main")
    changed = alice.workspaces.modify(
        repo=REPO,
        name="ws-2",
        message="edit and delete",
        changes=[put_file("README.md", "# acme\nmore\n"), delete_file("src/lib.rs")],
        expected_head=State.head,
    )
    p = alice.proposals.create(repo=REPO, stream="main", head=changed.head, title="Edit")
    bob.proposals.review(repo=REPO, id=p.id, decision="approve")
    assert carol.integration.integrate(repo=REPO, proposal=p.id).outcome.status == "integrated"
    diff = alice.tree.diff(repo=REPO, from_=State.head, to="main")
    assert sorted(f"{f.status}:{f.path}" for f in diff.files) == ["deleted:src/lib.rs", "modified:README.md"]
    with pytest.raises(NotFoundError):
        alice.tree.file(repo=REPO, path="src/lib.rs", rev="main")
    # Workspace revisions are addressable as @name.
    assert alice.tree.file(repo=REPO, path="README.md", rev="@ws-2") == b"# acme\nmore\n"
    alice.workspaces.delete(repo=REPO, name="ws-2")
    with pytest.raises(NotFoundError):
        alice.workspaces.get(repo=REPO, name="ws-2")


def test_a_client_that_builds_the_changeset_itself(team: dict[str, AgentGitClient]) -> None:
    alice, bob, carol = team["alice"], team["bob"], team["carol"]
    info = alice.repos.describe(repo=REPO)
    base = alice.streams.head(repo=REPO, stream="main").head
    built = build_changeset(
        namespace=info.namespace,
        parents=[base],
        author="user:alice",
        timestamp_ms=1_760_659_200_000,
        message="add docs (built by the client)",
        ops=[PutOp("docs/guide.md", content="# guide\n"), PutOp("docs/copy.md", content="# guide\n")],
    )
    assert len(built.blobs) == 1
    first = alice.objects.upload(repo=REPO, objects=built.objects())
    assert len(first.stored) == 2
    second = alice.objects.upload(repo=REPO, objects=built.objects())
    assert second.stored == []
    assert len(second.already_present) == 2
    # The server re-derives the id from the bytes: both must agree.
    assert alice.objects.get(repo=REPO, object_id=built.id) == built.object.envelope
    verified = alice.objects.verify(repo=REPO, object_ids=[built.id, *(b.id for b in built.blobs)])
    assert all(o.status == "ok" for o in verified.objects)

    proposal = alice.proposals.create(repo=REPO, stream="main", head=built.id, title="Docs")
    bob.proposals.review(repo=REPO, id=proposal.id, decision="approve")
    done = carol.integration.integrate(repo=REPO, proposal=proposal.id)
    assert (done.outcome.status, done.outcome.new_head) == ("integrated", built.id)
    assert alice.tree.file(repo=REPO, path="docs/guide.md", rev="main") == b"# guide\n"


def test_put_blob_and_the_server_checks_what_it_is_given(team: dict[str, AgentGitClient]) -> None:
    alice = team["alice"]
    res = alice.objects.put_blob(repo=REPO, content="hello\n")
    assert res.id == "6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545"
    assert not alice.objects.put_blob(repo=REPO, content="hello\n").stored
    with pytest.raises(ObjectCorruptError) as info:
        alice.objects.put(
            repo=REPO,
            object_id="ab" * 32,
            data=blob_object("some bytes").envelope,
            options=CallOptions(retry=False),
        )
    assert info.value.status == 422


def test_the_async_client_runs_the_same_life_cycle(server: RunningServer) -> None:
    async def scenario() -> None:
        repo = "acme/async-web"
        async with server.async_admin() as admin:
            await admin.repos.create(name=repo)
        async with (
            server.async_actor("user:ann", repo, "writer") as ann,
            server.async_actor("user:cy", repo, "integrator") as cy,
        ):
            await ann.workspaces.create(repo=repo, name="w", stream="main")
            changed = await ann.workspaces.modify(repo=repo, name="w", changes=[put_file("a.txt", "a\n")])
            p = await ann.proposals.create(repo=repo, stream="main", head=changed.head, title="a")
            done = await cy.integration.integrate(repo=repo, proposal=p.id)
            assert done.outcome.status == "integrated"
            assert await ann.tree.file(repo=repo, path="a.txt", rev="main") == b"a\n"
            assert [c.message async for c in ann.paginate("history.log", repo=repo, rev="main")] == [
                "Update workspace"
            ]
            up = await ann.objects.put_blob(repo=repo, content=b"\x00\x01binary")
            assert len(up.id) == 64

    asyncio.run(scenario())
