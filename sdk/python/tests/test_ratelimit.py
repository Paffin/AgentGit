"""Backpressure: the server rate-limits per credential and says how long to wait (429 + retry_after_ms)."""

from __future__ import annotations

import asyncio
import concurrent.futures
import time
from collections.abc import Iterator

import httpx
import pytest

from agentgit import AgentGitClient, QuotaExceededError, RateLimitedError, RetryPolicy
from tests.faults import FaultyTransport, Reply
from tests.harness import RunningServer, start_server

REPO = "acme/limited"


@pytest.fixture(scope="module")
def limited() -> Iterator[RunningServer]:
    s = start_server(config="[limits]\nrequests_per_second = 20\nburst = 5\n")
    with s.admin() as admin:
        admin.repos.create(name=REPO)
        admin.repos.create(name="acme/small")
    try:
        yield s
    finally:
        s.stop()


def test_without_retries_a_burst_surfaces_rate_limited_with_a_retry_hint(limited: RunningServer) -> None:
    with limited.actor("user:burst", REPO, "reader", retry=False) as reader:
        error: RateLimitedError | None = None
        for _ in range(30):
            try:
                reader.repos.describe(repo=REPO)
            except RateLimitedError as e:
                error = e
                break
    assert error is not None
    assert (error.code, error.status, error.retryable) == ("rate_limited", 429, True)
    assert 0 < error.retry_after_ms <= 1000


def test_the_default_policy_waits_as_told_and_every_request_succeeds(limited: RunningServer) -> None:
    net = FaultyTransport()
    waits: list[float] = []

    def sleep(seconds: float) -> None:
        waits.append(seconds)
        time.sleep(seconds)

    with limited.actor(
        "user:patient",
        REPO,
        "reader",
        http_client=httpx.Client(transport=net),
        retry=RetryPolicy(max_attempts=10),
        sleep=sleep,
    ) as reader:
        started = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            names = list(pool.map(lambda _: reader.repos.describe(repo=REPO).name, range(16)))
    assert names == [REPO] * 16
    # 16 requests, burst 5, 20 per second: some had to be told to wait, and the client did.
    assert waits
    assert time.monotonic() - started > 0.5
    assert len(net.seen) > 16


def test_a_mutation_the_server_cannot_deduplicate_is_still_retried_on_429_and_applied_once(
    limited: RunningServer,
) -> None:
    with limited.actor("user:tasker", REPO, "writer", retry=RetryPolicy(max_attempts=10)) as writer:
        titles = [f"task {i}" for i in range(12)]
        with concurrent.futures.ThreadPoolExecutor(6) as pool:
            list(pool.map(lambda t: writer.tasks.create(repo=REPO, title=t), titles))
        assert sorted(t.title for t in writer.tasks.list(repo=REPO).tasks) == sorted(titles)


def test_an_unsafe_mutation_hit_by_another_transient_error_is_not_retried(limited: RunningServer) -> None:
    net = FaultyTransport()
    net.match = lambda r: r.path.endswith("/tasks") and r.method == "POST"
    net.rule = lambda _r, _n: Reply(
        503,
        {"content-type": "application/json"},
        '{"error":{"code":"unavailable","message":"busy","retryable":true,"retry_after_ms":1}}',
    )
    with AgentGitClient(
        limited.base_url,
        limited.bootstrap_token,
        http_client=httpx.Client(transport=net),
        sleep=lambda _s: None,
    ) as client:
        with pytest.raises(Exception) as info:
            client.tasks.create(repo=REPO, title="no retry")
    assert getattr(info.value, "attempts", None) == 1
    assert len(net.seen) == 1


def test_quota_exceeded_is_a_429_that_waiting_does_not_fix(limited: RunningServer) -> None:
    with limited.admin(retry=False) as admin:
        admin.repos.configure(repo="acme/small", quota={"max_streams": 2})
    with limited.actor("user:quota", "acme/small", "writer", retry=RetryPolicy(max_attempts=5)) as writer:
        writer.streams.create(repo="acme/small", name="dev")
        with pytest.raises(QuotaExceededError) as info:
            writer.streams.create(repo="acme/small", name="dev2")
    e = info.value
    assert (e.status, e.retryable, e.attempts) == (429, False, 1)


def test_the_async_client_waits_as_told_too(limited: RunningServer) -> None:
    async def scenario() -> None:
        async with limited.async_actor(
            "user:async-patient", REPO, "reader", retry=RetryPolicy(max_attempts=10)
        ) as reader:
            results = await asyncio.gather(*(reader.repos.describe(repo=REPO) for _ in range(14)))
        assert {r.name for r in results} == {REPO}

    asyncio.run(scenario())
