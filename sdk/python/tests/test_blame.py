"""history.blame against a real server: which changeset introduced each line, line ranges, typed errors."""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from agentgit import AgentGitClient, AgentGitError, put_file
from tests.harness import RunningServer

REPO = "blame/lines"


@dataclass
class Fixture:
    alice: AgentGitClient
    first: str
    second: str


@pytest.fixture(scope="module")
def blamed(server: RunningServer) -> Fixture:
    admin = server.admin()
    admin.repos.create(name=REPO)
    alice = server.actor("user:alice", REPO, "writer")

    # Two changesets land on main directly (stream.cas needs the admin role; no proposal is needed for a test).
    base = admin.streams.head(repo=REPO, stream="main")
    alice.workspaces.create(repo=REPO, name="w", stream="main")
    m1 = alice.workspaces.modify(
        repo=REPO, name="w", message="first", changes=[put_file("x.txt", "one\ntwo\nthree\n")]
    )
    alice.objects.publish(repo=REPO, head=m1.head)
    c1 = admin.streams.cas(
        repo=REPO,
        stream="main",
        expected_version=base.version,
        expected_head=base.head,
        new_head=m1.head,
        reason="blame test",
    )
    m2 = alice.workspaces.modify(
        repo=REPO, name="w", message="second", changes=[put_file("x.txt", "one\nTWO\nthree\n")]
    )
    alice.objects.publish(repo=REPO, head=m2.head)
    admin.streams.cas(
        repo=REPO,
        stream="main",
        expected_version=c1.version,
        expected_head=m1.head,
        new_head=m2.head,
        reason="blame test",
    )
    return Fixture(alice, m1.head, m2.head)


def test_attributes_every_line_to_the_changeset_that_last_wrote_it(blamed: Fixture) -> None:
    blame = blamed.alice.history.blame(repo=REPO, path="x.txt", rev="main")
    assert blame.rev == blamed.second
    assert blame.truncated is False
    assert [(ln.line, ln.text, ln.changeset, ln.summary) for ln in blame.lines] == [
        (1, "one", blamed.first, "first"),
        (2, "TWO", blamed.second, "second"),
        (3, "three", blamed.first, "first"),
    ]
    assert all(ln.path == "x.txt" and ln.author == "user:alice" for ln in blame.lines)


def test_honours_line_ranges_and_a_revision_other_than_the_tip(blamed: Fixture) -> None:
    ranged = blamed.alice.history.blame(repo=REPO, path="x.txt", first_line=2, last_line=3)
    assert [ln.line for ln in ranged.lines] == [2, 3]
    before = blamed.alice.history.blame(repo=REPO, path="x.txt", rev=blamed.first)
    assert [ln.text for ln in before.lines] == ["one", "two", "three"]
    assert {ln.changeset for ln in before.lines} == {blamed.first}


def test_a_missing_file_is_a_typed_error(blamed: Fixture) -> None:
    with pytest.raises(AgentGitError) as info:
        blamed.alice.history.blame(repo=REPO, path="nope.txt")
    assert info.value.retryable is False
    assert info.value.status >= 400
