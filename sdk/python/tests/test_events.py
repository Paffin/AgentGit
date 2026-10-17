"""Server-Sent Events against a real server: replay, live follow, heartbeats, reconnect with resume."""

from __future__ import annotations

import asyncio
import threading
import time
from collections.abc import Iterator
from contextlib import aclosing
from typing import Any

import httpx
import pytest

from agentgit import (
    AgentGitClient,
    AsyncAgentGitClient,
    NotFoundError,
    ReconnectPolicy,
    SseParser,
    UnauthenticatedError,
)
from agentgit.models import Event
from tests.faults import AsyncBreakingTransport, BreakingTransport
from tests.harness import RunningServer

REPO = "acme/events"


@pytest.fixture(scope="module")
def alice(server: RunningServer) -> AgentGitClient:
    with server.admin() as admin:
        admin.repos.create(name=REPO)
    return server.actor("user:alice", REPO, "writer")


def make_events(client: AgentGitClient, prefix: str, n: int) -> None:
    """Each workspace creation appends one ``workspace.create`` event."""
    for i in range(n):
        client.workspaces.create(repo=REPO, name=f"{prefix}-{i}", stream="main")


def take(it: Iterator[Event], n: int, seconds: float = 15) -> list[Event]:
    out: list[Event] = []
    deadline = time.monotonic() + seconds
    for ev in it:
        out.append(ev)
        if len(out) == n or time.monotonic() > deadline:
            break
    return out


def test_after_seq_zero_replays_the_whole_history_in_order(alice: AgentGitClient) -> None:
    make_events(alice, "replay", 3)
    replayed = alice.events.replay(repo=REPO, after_seq=0, limit=1000)
    total = len(replayed.events)
    assert total >= 4  # repo.init + 3 workspace.create
    seen = []
    for ev in alice.subscribe_events(repo=REPO, after_seq=0):
        seen.append(ev.seq)
        if len(seen) == total:
            break
    assert seen == [e.seq for e in replayed.events]


def test_resumes_after_a_given_seq_and_filters_by_topic_prefix(alice: AgentGitClient) -> None:
    everything = alice.events.replay(repo=REPO, after_seq=0, limit=1000).events
    first = everything[0]
    want = [e for e in everything if e.topic.startswith("workspace") and e.seq > first.seq]
    got = take(alice.subscribe_events(repo=REPO, after_seq=first.seq, topics=["workspace"]), len(want))
    assert all(e.topic.startswith("workspace.") and e.seq > first.seq for e in got)
    assert len(got) >= 3


def test_live_events_arrive_in_order_and_heartbeats_are_never_yielded(alice: AgentGitClient) -> None:
    make_events(alice, "before-live", 1)
    feed = alice.subscribe_events(repo=REPO, heartbeat_ms=50)
    got: list[Event] = []
    done = threading.Event()

    def consume() -> None:
        got.extend(take(feed, 2))
        done.set()

    threading.Thread(target=consume, daemon=True).start()
    time.sleep(0.5)  # several heartbeats pass without producing anything
    assert got == []
    make_events(alice, "live", 2)
    assert done.wait(10)
    assert [e.payload["target"] for e in got] == ["live-0", "live-1"]
    assert all(e.topic == "workspace.create" and e.repo == REPO for e in got)


def test_a_caller_without_access_gets_a_typed_error_at_the_first_read(
    server: RunningServer, alice: AgentGitClient
) -> None:
    eve = server.actor("user:eve", "other/*", "reader")
    with pytest.raises(NotFoundError):
        next(eve.subscribe_events(repo=REPO, after_seq=0))


def test_the_same_events_the_replay_api_returns(alice: AgentGitClient) -> None:
    one = alice.events.replay(repo=REPO, after_seq=0, limit=1).events[0]
    (via_sse,) = take(alice.subscribe_events(repo=REPO, after_seq=0), 1)
    assert via_sse == one
    assert "actor" in one.payload


@pytest.mark.parametrize("how", ["close", "error"])
def test_reconnect_resumes_without_gaps_or_duplicates(
    server: RunningServer, alice: AgentGitClient, how: str
) -> None:
    make_events(alice, f"rc-{how}", 6)
    expected = [e.seq for e in alice.events.replay(repo=REPO, after_seq=0, limit=1000).events]
    transport = BreakingTransport([(2, how), (2, how)])
    with AgentGitClient(
        server.base_url,
        server.bootstrap_token,
        http_client=httpx.Client(transport=transport),
        sleep=lambda _s: None,
    ) as client:
        seqs = []
        for ev in client.subscribe_events(repo=REPO, after_seq=0):
            seqs.append(ev.seq)
            if len(seqs) == len(expected):
                break
    assert seqs == expected
    # The first connection replays from 0; each reconnect asks for events after the last one it got.
    assert len(transport.urls) >= 3
    after = [int(httpx.URL(u).params["after_seq"]) for u in transport.urls]
    assert after[0] == 0
    assert after[1] > 0
    assert after == sorted(after)
    assert after[1] in expected


def test_a_non_retryable_error_stops_the_subscription(server: RunningServer, alice: AgentGitClient) -> None:
    short = server.actor("user:shorty", REPO, "reader")
    with server.admin() as admin:
        admin.credentials.revoke(credential_id=short.session.whoami().credential_id)
    with pytest.raises(UnauthenticatedError):
        next(short.subscribe_events(repo=REPO, after_seq=0))


def test_the_idle_timeout_reconnects_a_connection_that_goes_quiet() -> None:
    import socket

    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    port = listener.getsockname()[1]
    accepted: list[socket.socket] = []

    def serve() -> None:
        conn, _ = listener.accept()
        accepted.append(conn)
        conn.recv(4096)
        conn.sendall(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"
        )
        # ... and then nothing, not even a heartbeat. The second connection is refused by closing the listener.
        time.sleep(2)
        listener.close()

    threading.Thread(target=serve, daemon=True).start()
    slept: list[float] = []
    with AgentGitClient(f"http://127.0.0.1:{port}", "x", sleep=slept.append) as client:
        feed = client.subscribe_events(
            repo="a/b", after_seq=5, idle_timeout_ms=300, reconnect=ReconnectPolicy(max_attempts=1)
        )
        started = time.monotonic()
        with pytest.raises(Exception) as info:
            next(feed)
    # The silent connection was dropped by the watchdog (after ~0.3 s), then the reconnect failed.
    assert time.monotonic() - started < 5
    assert slept  # it backed off before trying again
    assert getattr(info.value, "code", "") == "unavailable"
    for c in accepted:
        c.close()


def test_sse_parser_handles_chunk_boundaries_and_line_ends() -> None:
    def frames(*chunks: bytes) -> list[Any]:
        p = SseParser()
        out = []
        for c in chunks:
            out += p.feed(c)
        out += p.feed(b"", final=True)
        return out

    assert [
        (f.event, f.data, f.id)
        for f in frames(b": heartbeat\n\n", b"id: 7\nevent: stream.integrate\ndata: {}\n\n")
    ] == [("stream.integrate", "{}", "7")]
    assert [f.data for f in frames(b"id: 1\r\ndata: a\r\ndata: b\r\n\r\n")] == ["a\nb"]
    assert [f.data for f in frames(b"data: x\r\r")] == ["x"]
    assert [f.data for f in frames(b"data: y\r", b"\n\r", b"\n")] == ["y"]
    raw = 'id: 3\nevent: x\ndata: {"t":"Привет ☃ 🦀"}\n\n'.encode()
    for cut in range(1, len(raw), 3):
        got = frames(raw[:cut], raw[cut:])
        assert len(got) == 1
        assert got[0].data == '{"t":"Привет ☃ 🦀"}'
    got = frames("\ufeffid: 1\ndata: a\n\n".encode(), b"data: b\nretry: 1500\n\n", b"data: never finished")
    assert [(f.data, f.id, f.retry) for f in got] == [("a", "1", None), ("b", "1", 1500)]


def test_the_async_subscription_replays_follows_and_reconnects(
    server: RunningServer, alice: AgentGitClient
) -> None:
    make_events(alice, "async-rc", 5)
    expected = [e.seq for e in alice.events.replay(repo=REPO, after_seq=0, limit=1000).events]

    async def scenario() -> None:
        transport = AsyncBreakingTransport([(3, "error"), (3, "close")])
        async with AsyncAgentGitClient(
            server.base_url,
            server.bootstrap_token,
            http_client=httpx.AsyncClient(transport=transport),
            sleep=lambda _s: asyncio.sleep(0),
        ) as client:
            seqs: list[int] = []
            async with aclosing(client.subscribe_events(repo=REPO, after_seq=0)) as replay:
                async for ev in replay:
                    seqs.append(ev.seq)
                    if len(seqs) == len(expected):
                        break
            assert seqs == expected
            assert len(transport.urls) >= 3

        # live, from "now"
        async with server.async_actor("user:async-alice", REPO, "writer") as live:
            feed = live.subscribe_events(repo=REPO, heartbeat_ms=50)
            got: list[Event] = []

            async def consume() -> None:
                async with aclosing(feed):
                    async for ev in feed:
                        got.append(ev)
                        if len(got) == 2:
                            return

            task = asyncio.create_task(consume())
            await asyncio.sleep(0.4)
            for i in range(2):
                await live.workspaces.create(repo=REPO, name=f"async-live-{i}", stream="main")
            await asyncio.wait_for(task, 10)
            assert [e.payload["target"] for e in got] == ["async-live-0", "async-live-1"]

    asyncio.run(scenario())
