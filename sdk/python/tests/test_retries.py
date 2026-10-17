"""Retries, idempotency keys and deadlines against a real server, with faults injected between the
client and the server: a lost response must never apply a request twice."""

from __future__ import annotations

import asyncio
import socket
import threading
import time
from collections.abc import Callable
from typing import Any

import httpx
import pytest

from agentgit import (
    AgentGitClient,
    AgentGitError,
    AlreadyExistsError,
    AsyncAgentGitClient,
    CallOptions,
    DeadlineExceededError,
    FailedPreconditionError,
    IdempotencyKeyReusedError,
    NotFoundError,
    RateLimitedError,
    RetryPolicy,
    StreamMovedError,
    UnavailableError,
    generate_agent_key,
    put_file,
)
from tests.faults import AsyncFaultyTransport, FaultyTransport, RecordedSleep, Reply
from tests.harness import RunningServer

REPO = "acme/retry"


@pytest.fixture(scope="module", autouse=True)
def _repo(server: RunningServer) -> None:
    with server.admin() as admin:
        admin.repos.create(name=REPO)


class Setup:
    def __init__(self, server: RunningServer, **options: Any) -> None:
        self.net = FaultyTransport()
        self.sleep = RecordedSleep()
        self.server = server
        self.client = AgentGitClient(
            server.base_url,
            server.bootstrap_token,
            http_client=httpx.Client(transport=self.net),
            sleep=self.sleep,
            random=lambda: 0.5,
            **options,
        )

    def actor(self, subject: str, role: str) -> AgentGitClient:
        return self.server.actor(
            subject,
            REPO,
            role,
            http_client=httpx.Client(transport=self.net),
            sleep=self.sleep,
            random=lambda: 0.5,
        )


@pytest.fixture
def setup(server: RunningServer) -> Setup:
    s = Setup(server)
    return s


def unavailable(retry_after_ms: int) -> Reply:
    headers = {"content-type": "application/json"}
    if retry_after_ms > 0:
        headers["retry-after"] = "2"
    body = (
        '{"error":{"code":"unavailable","message":"busy","retryable":true,'
        f'"retry_after_ms":{retry_after_ms},"request_id":"req_x","details":{{}}}}}}'
    )
    return Reply(503, headers, body)


class TestWhichRequestsAreRetried:
    def test_reads_are_retried_and_the_server_delay_is_a_floor(self, setup: Setup) -> None:
        setup.net.rule = lambda _r, n: unavailable(120) if n <= 2 else "pass"
        assert setup.client.repos.describe(repo=REPO).name == REPO
        assert len(setup.net.seen) == 3
        # retry_after_ms (from the body) wins over the coarser Retry-After header and is a floor;
        # jitter (random() = 0.5 of the 200 ms / 400 ms window) is added on top
        assert setup.sleep.delays == [0.22, 0.32]
        ids = [s.headers["x-request-id"] for s in setup.net.seen]
        assert ids[1] == f"{ids[0]}-r2"

    def test_retry_after_header_and_exponential_backoff(self, server: RunningServer) -> None:
        a = Setup(server)
        a.net.rule = lambda _r, n: (
            Reply(503, {"retry-after": "2"}, "Service Unavailable") if n == 1 else "pass"
        )
        a.client.repos.describe(repo=REPO)
        assert a.sleep.delays == [2.1]

        b = Setup(server, retry=RetryPolicy(max_attempts=5, base_delay_ms=100, max_delay_ms=350))
        b.net.rule = lambda _r, n: unavailable(0) if n <= 4 else "pass"
        b.client.repos.describe(repo=REPO)
        assert b.sleep.delays == [0.05, 0.1, 0.175, 0.175]  # full jitter of min(max, base * 2^(attempt-1))

    def test_gives_up_after_max_attempts_and_reports_the_attempts(self, server: RunningServer) -> None:
        s = Setup(server, retry=RetryPolicy(max_attempts=3))
        s.net.rule = lambda _r, _n: unavailable(1)
        with pytest.raises(UnavailableError) as info:
            s.client.repos.describe(repo=REPO)
        assert (info.value.status, info.value.retryable, info.value.attempts) == (503, True, 3)
        assert len(s.net.seen) == 3

    def test_does_not_retry_what_the_server_did_not_mark_retryable(self, setup: Setup) -> None:
        calls: list[tuple[Callable[[], Any], type[AgentGitError]]] = [
            (lambda: setup.client.repos.describe(repo="acme/nope"), NotFoundError),
            (lambda: setup.client.repos.create(name=REPO), AlreadyExistsError),
        ]
        for call, exc in calls:
            setup.net.reset()
            with pytest.raises(exc) as info:
                call()
            assert info.value.attempts == 1
            assert len(setup.net.seen) == 1

    def test_a_lost_compare_and_swap_is_not_retried(self, setup: Setup) -> None:
        c = setup.client
        c.streams.create(repo=REPO, name="cas")
        c.workspaces.create(repo=REPO, name="cas-ws", stream="cas")
        ws = c.workspaces.modify(repo=REPO, name="cas-ws", changes=[put_file("c.txt", "c")])
        c.proposals.create(repo=REPO, stream="cas", head=ws.head, title="publishes the head")
        setup.net.reset()
        with pytest.raises(StreamMovedError) as info:
            c.streams.cas(repo=REPO, stream="cas", expected_version=7, expected_head="", new_head=ws.head)
        # The server calls this retryable, but the identical request can never win: it is not repeated.
        assert (info.value.status, info.value.retryable, info.value.attempts) == (409, True, 1)
        assert len(setup.net.seen) == 1

    def test_refuses_to_wait_for_an_absurd_retry_after(self, server: RunningServer) -> None:
        s = Setup(server, retry=RetryPolicy(max_retry_after_ms=5_000))
        s.net.rule = lambda _r, _n: Reply(
            429,
            {"content-type": "application/json"},
            '{"error":{"code":"rate_limited","message":"slow","retryable":true,"retry_after_ms":3600000}}',
        )
        with pytest.raises(RateLimitedError) as info:
            s.client.repos.describe(repo=REPO)
        assert (info.value.retry_after_ms, info.value.attempts) == (3_600_000, 1)
        assert len(s.net.seen) == 1


class TestIdempotencyKeys:
    def test_a_key_is_generated_for_keyed_mutations_and_none_for_reads(self, setup: Setup) -> None:
        setup.client.repos.describe(repo=REPO)
        setup.client.repos.create(name="acme/keys-1")
        setup.client.repos.create(name="acme/keys-2")
        read, a, b = setup.net.seen
        assert "idempotency-key" not in read.headers
        assert a.headers["idempotency-key"].startswith("sdk-")
        assert len(a.headers["idempotency-key"]) == 28
        assert b.headers["idempotency-key"] != a.headers["idempotency-key"]

    def test_an_explicit_key_is_used_as_given(self, setup: Setup) -> None:
        setup.client.repos.create(name="acme/keys-3", options=CallOptions(idempotency_key="mine-1"))
        setup.client.repos.create(name="acme/keys-4", idempotency_key="mine-2")
        assert [s.headers.get("idempotency-key") for s in setup.net.seen] == ["mine-1", "mine-2"]

    def test_a_key_reused_for_a_different_request_is_refused_and_never_retried(self, setup: Setup) -> None:
        setup.client.repos.create(name="acme/keys-5", options=CallOptions(idempotency_key="dup-key"))
        setup.net.reset()
        with pytest.raises(IdempotencyKeyReusedError) as info:
            setup.client.repos.create(name="acme/keys-6", options=CallOptions(idempotency_key="dup-key"))
        assert (info.value.status, info.value.retryable) == (422, False)
        assert len(setup.net.seen) == 1

    def test_repo_create_lost_response_is_retried_with_the_same_key(self, setup: Setup) -> None:
        setup.net.match = lambda r: r.method == "POST" and r.path == "/v1/repos"
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        assert setup.client.repos.create(name="acme/lost-1").name == "acme/lost-1"
        posts = setup.net.to("/v1/repos", "POST")
        assert len(posts) == 2
        assert posts[1].headers["idempotency-key"] == posts[0].headers["idempotency-key"]
        assert posts[1].body == posts[0].body
        assert len(setup.client.repos.list(prefix="acme/lost-1").repos) == 1

    def test_proposal_create_and_integrate_lost_responses_never_duplicate_the_effect(
        self, setup: Setup
    ) -> None:
        alice, carol = setup.actor("user:alice", "writer"), setup.actor("user:carol", "integrator")
        alice.workspaces.create(repo=REPO, name="ws", stream="main")
        changed = alice.workspaces.modify(repo=REPO, name="ws", changes=[put_file("a.txt", "a\n")])

        setup.net.match = lambda r: r.method == "POST" and r.path.endswith("/proposals")
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        proposal = alice.proposals.create(repo=REPO, stream="main", head=changed.head, title="lost response")
        titles = [p.title for p in alice.proposals.list(repo=REPO).proposals]
        assert titles.count("lost response") == 1

        setup.net.reset()
        setup.net.match = lambda r: r.method == "POST" and r.path.endswith("/integrate")
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        done = carol.integration.integrate(repo=REPO, proposal=proposal.id)
        assert done.outcome.status == "integrated"
        posts = [s for s in setup.net.seen if s.path.endswith("/integrate")]
        assert len(posts) == 2
        assert posts[0].headers["idempotency-key"] == posts[1].headers["idempotency-key"]
        reflog = carol.streams.reflog(repo=REPO, stream="main")
        assert [e.new_head for e in reflog.entries].count(
            changed.head
        ) == 1  # one integration, one reflog entry
        assert carol.streams.head(repo=REPO, stream="main").version == 1

    def test_task_run_and_lease_lost_responses_are_retried_with_the_same_key_and_applied_once(
        self, setup: Setup
    ) -> None:
        bot = setup.actor("agent:retry-bot", "writer")
        bot.agents.register(repo=REPO, name="retry-bot", public_key=generate_agent_key().public_key_hex)

        def lost_once(suffix: str) -> None:
            setup.net.reset()
            setup.net.match = lambda r: r.method == "POST" and r.path.endswith(suffix)
            setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"

        def same_key_twice(suffix: str) -> None:
            posts = [s for s in setup.net.seen if s.method == "POST" and s.path.endswith(suffix)]
            assert len(posts) == 2
            assert posts[0].headers["idempotency-key"].startswith("sdk-")
            assert posts[1].headers["idempotency-key"] == posts[0].headers["idempotency-key"]
            assert posts[1].body == posts[0].body

        lost_once("/tasks")
        task = bot.tasks.create(repo=REPO, title="created once")
        same_key_twice("/tasks")
        assert [t.id for t in bot.tasks.list(repo=REPO).tasks if t.title == "created once"] == [task.id]

        lost_once("/runs")
        run = bot.agents.start_run(repo=REPO, agent="retry-bot", task_id=task.id)
        same_key_twice("/runs")
        assert [r.run_id for r in bot.agents.list_runs(repo=REPO, agent="retry-bot").runs] == [run.run_id]

        lost_once("/leases")
        lease = bot.leases.acquire(repo=REPO, scope="retry/**", exclusive=True, ttl_ms=60_000)
        same_key_twice("/leases")
        # the retry did not collide with the first attempt's own lease, and only one lease exists
        active = [x for x in bot.leases.list(repo=REPO, active_only=True).leases if x.scope == "retry/**"]
        assert [x.id for x in active] == [lease.lease.id]

    def test_objects_put_is_naturally_idempotent_and_retried(self, setup: Setup) -> None:
        setup.net.match = lambda r: r.method == "PUT"
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        res = setup.client.objects.put_blob(repo=REPO, content="lost put\n")
        assert len([s for s in setup.net.seen if s.method == "PUT"]) == 2
        assert not res.stored  # the first attempt had already stored it


class TestRequestsTheServerCannotDeduplicate:
    # credentials.mint ignores Idempotency-Key by design (the secret can only be shown once), so a repeat
    # would mint a second credential.
    @staticmethod
    def minted(client: AgentGitClient, subject: str) -> int:
        return len([c for c in client.credentials.list(subject=subject).credentials if c.subject == subject])

    def test_are_not_retried_after_a_lost_response(self, setup: Setup) -> None:
        setup.net.match = lambda r: r.method == "POST" and r.path == "/v1/credentials"
        setup.net.rule = lambda _r, _n: "drop-response"
        with pytest.raises(UnavailableError) as info:
            setup.client.credentials.mint(subject="user:maybe-twice", ttl_secs=60)
        e = info.value
        assert (e.status, e.retryable, e.outcome_unknown, e.attempts) == (0, True, True, 1)
        assert len(setup.net.to("/v1/credentials", "POST")) == 1
        assert self.minted(setup.client, "user:maybe-twice") == 1  # applied once

    def test_are_retried_only_on_request_and_then_may_be_applied_twice(self, setup: Setup) -> None:
        setup.net.match = lambda r: r.method == "POST" and r.path == "/v1/credentials"
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        setup.client.credentials.mint(
            subject="user:forced-retry",
            ttl_secs=60,
            options=CallOptions(retry=RetryPolicy(retry_unsafe=True)),
        )
        assert self.minted(setup.client, "user:forced-retry") == 2

    def test_workspace_modify_becomes_retryable_with_expected_head(self, setup: Setup) -> None:
        alice = setup.actor("user:wally", "writer")
        ws = alice.workspaces.create(repo=REPO, name="guarded", stream="main")
        setup.net.match = lambda r: r.method == "POST" and r.path.endswith("/guarded/changes")
        setup.net.reset()
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        # Without the guard a lost response is not retried ...
        with pytest.raises(UnavailableError) as unguarded:
            alice.workspaces.modify(repo=REPO, name="guarded", changes=[put_file("x", "1")])
        assert (unguarded.value.outcome_unknown, unguarded.value.attempts) == (True, 1)
        # ... with it, the retry runs (and is refused, because the first attempt already moved the head).
        setup.net.reset()
        setup.net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
        current = alice.workspaces.get(repo=REPO, name="guarded").head
        assert current != ws.head
        with pytest.raises(FailedPreconditionError) as guarded:
            alice.workspaces.modify(
                repo=REPO, name="guarded", changes=[put_file("y", "2")], expected_head=current
            )
        assert guarded.value.attempts == 2
        assert [e.path for e in alice.tree.get(repo=REPO, rev="@guarded").entries].count("y") == 1


class TestDeadlines:
    def test_the_deadline_is_sent_and_the_server_answers_deadline_exceeded(
        self, server: RunningServer
    ) -> None:
        s = Setup(server, retry=False)
        started = time.monotonic()
        with pytest.raises(DeadlineExceededError) as info:
            s.client.streams.head(
                repo=REPO,
                stream="main",
                wait_version_gt=9999,
                wait_ms=20_000,
                options=CallOptions(timeout_ms=300),
            )
        assert time.monotonic() - started < 5
        assert s.net.seen[0].headers["request-timeout"] == "300"
        e = info.value
        assert (e.status, e.retryable, e.outcome_unknown) == (504, True, True)

    def test_a_deadline_is_retried_for_reads(self, server: RunningServer) -> None:
        s = Setup(server, retry=RetryPolicy(max_attempts=2))
        with pytest.raises(DeadlineExceededError) as info:
            s.client.streams.head(
                repo=REPO,
                stream="main",
                wait_version_gt=9999,
                wait_ms=20_000,
                options=CallOptions(timeout_ms=200),
            )
        assert info.value.attempts == 2
        assert len(s.net.seen) == 2

    def test_a_long_poll_gets_a_deadline_longer_than_its_wait(self, setup: Setup) -> None:
        head = setup.client.streams.head(repo=REPO, stream="main", wait_version_gt=9999, wait_ms=150)
        assert head.name == "main"
        assert int(setup.net.seen[0].headers["request-timeout"]) > 10_000

    def test_the_deadline_is_enforced_locally_when_the_server_never_answers(self) -> None:
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        held: list[socket.socket] = []
        stop = threading.Event()

        def accept() -> None:
            listener.settimeout(0.2)
            while not stop.is_set():
                try:
                    held.append(listener.accept()[0])
                except OSError:
                    continue

        t = threading.Thread(target=accept, daemon=True)
        t.start()
        try:
            port = listener.getsockname()[1]
            with AgentGitClient(f"http://127.0.0.1:{port}", "x", retry=False) as client:
                started = time.monotonic()
                with pytest.raises(DeadlineExceededError) as info:
                    client.session.whoami(options=CallOptions(timeout_ms=250))
                assert time.monotonic() - started < 3
                assert (info.value.status, info.value.retryable, info.value.outcome_unknown) == (
                    0,
                    True,
                    True,
                )
        finally:
            stop.set()
            t.join(2)
            for s in held:
                s.close()
            listener.close()

    def test_a_refused_connection_is_a_retryable_transport_error(self) -> None:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        with AgentGitClient(
            f"http://127.0.0.1:{port}", "x", retry=RetryPolicy(max_attempts=2), sleep=lambda _s: None
        ) as c:
            with pytest.raises(UnavailableError) as info:
                c.session.whoami()
        assert (info.value.status, info.value.attempts) == (0, 2)


def test_the_async_client_applies_the_same_rules(server: RunningServer) -> None:
    async def scenario() -> None:
        net = AsyncFaultyTransport()
        sleep = RecordedSleep()
        async with AsyncAgentGitClient(
            server.base_url,
            server.bootstrap_token,
            http_client=httpx.AsyncClient(transport=net),
            sleep=sleep.asleep,
            random=lambda: 0.5,
        ) as client:
            # reads: retried, server delay is a floor
            net.rule = lambda _r, n: unavailable(120) if n <= 2 else "pass"
            assert (await client.repos.describe(repo=REPO)).name == REPO
            assert sleep.delays == [0.22, 0.32]

            # keyed: lost response retried with the same key, applied once
            net.reset()
            net.match = lambda r: r.method == "POST" and r.path == "/v1/repos"
            net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
            await client.repos.create(name="acme/async-lost")
            posts = net.to("/v1/repos", "POST")
            assert len(posts) == 2
            assert posts[0].headers["idempotency-key"] == posts[1].headers["idempotency-key"]
            assert len((await client.repos.list(prefix="acme/async-lost")).repos) == 1

            # unsafe: not retried, outcome unknown
            net.reset()
            net.match = lambda r: r.method == "POST" and r.path == "/v1/credentials"
            net.rule = lambda _r, _n: "drop-response"
            with pytest.raises(AgentGitError) as info:
                await client.credentials.mint(subject="user:async-maybe-twice", ttl_secs=60)
            assert (info.value.outcome_unknown, info.value.attempts) == (True, 1)
            listed = (await client.credentials.list(subject="user:async-maybe-twice")).credentials
            assert len(listed) == 1

            # keyed since the server records tasks under the key: a lost response is retried, applied once
            net.reset()
            net.match = lambda r: r.method == "POST" and r.path.endswith("/tasks")
            net.rule = lambda _r, n: "drop-response" if n == 1 else "pass"
            task = await client.tasks.create(repo=REPO, title="async created once")
            posts = net.to(f"/v1/repos/{REPO}/tasks", "POST")
            assert len(posts) == 2
            assert posts[0].headers["idempotency-key"] == posts[1].headers["idempotency-key"]
            tasks = (await client.tasks.list(repo=REPO)).tasks
            assert [t.id for t in tasks if t.title == "async created once"] == [task.id]

            # deadline
            net.reset()
            net.match = lambda _r: True
            net.rule = lambda _r, _n: "pass"
            with pytest.raises(DeadlineExceededError) as late:
                await client.streams.head(
                    repo=REPO,
                    stream="main",
                    wait_version_gt=9999,
                    wait_ms=20_000,
                    options=CallOptions(timeout_ms=200, retry=False),
                )
            assert late.value.status == 504

    asyncio.run(scenario())
