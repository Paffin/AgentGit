"""Event subscriptions over Server-Sent Events, with reconnect and resume.

The subscription streams with ``httpx`` (so the ``Authorization`` header works), and when the
connection drops, the server closes the stream, or nothing - not even a heartbeat - arrives for the
idle timeout, it reconnects with ``after_seq`` set to the last delivered ``seq``. A ``seq`` is never
delivered twice and heartbeats are never yielded.
"""

from __future__ import annotations

import json
import time
from collections.abc import AsyncGenerator, Generator, Sequence
from typing import Any

import httpx

from agentgit._async import AsyncTransport
from agentgit._codec import decode_as
from agentgit._generated.models import Event
from agentgit._generated.operations import OPERATIONS
from agentgit._options import CallOptions, ReconnectPolicy
from agentgit._sync import SyncTransport
from agentgit.errors import AgentGitError, UnavailableError, error_from_info
from agentgit.sse import SseFrame, SseParser

DEFAULT_IDLE_MS = 90_000
# A connection that stayed up this long counts as established: the failure counter of the reconnect policy restarts.
STABLE_CONNECTION_S = 5
# "after_seq" of a replay that is certainly past the end of the log: the answer's latest_seq is the
# current end. (2**53 - 1 is the largest integer every JSON implementation carries exactly.)
_PAST_THE_END = 2**53 - 1


def _params(repo: str, last: int, topics: Sequence[str] | None, heartbeat_ms: int | None) -> dict[str, Any]:
    params: dict[str, Any] = {"repo": repo, "after_seq": last}
    if topics:
        params["topics"] = list(topics)
    if heartbeat_ms is not None:
        params["heartbeat_ms"] = heartbeat_ms
    return params


def _event(frame: SseFrame) -> Event:
    """The event of a frame, or raises the error an ``error`` frame carries."""
    if frame.event == "error":
        info = json.loads(frame.data).get("error") or {}
        raise error_from_info(info, status=0, operation="events.subscribe")
    return decode_as(Event, json.loads(frame.data))


def _idle_seconds(idle_timeout_ms: int | None) -> float | None:
    """``None`` (the default) is 90 s; ``0`` turns the idle watchdog off, as ``idleTimeoutMs: 0`` does in TypeScript."""
    if idle_timeout_ms is None:
        return DEFAULT_IDLE_MS / 1000
    return idle_timeout_ms / 1000 if idle_timeout_ms > 0 else None


def _backoff(policy: ReconnectPolicy, failures: int, rand: float) -> float:
    ceiling = min(policy.max_delay_ms, policy.base_delay_ms * (1 << min(max(0, failures - 1), 30)))
    return round(rand * ceiling) / 1000


def subscribe_sync(
    transport: SyncTransport,
    *,
    repo: str,
    after_seq: int | None,
    topics: Sequence[str] | None,
    heartbeat_ms: int | None,
    idle_timeout_ms: int | None,
    reconnect: ReconnectPolicy | None,
    run_id: str | None,
) -> Generator[Event, None, None]:
    op = OPERATIONS["events.subscribe"]
    policy = reconnect or ReconnectPolicy()
    idle_s = _idle_seconds(idle_timeout_ms)
    last = after_seq
    if last is None:
        # "From now": pin down the current end of the log, so that a reconnect cannot skip events
        # appended between two connections.
        probe = transport.execute(
            OPERATIONS["events.replay"],
            {"repo": repo, "after_seq": _PAST_THE_END, "limit": 1},
            None,
            CallOptions(run_id=run_id),
        )
        last = int(probe.get("latest_seq", 0))
    failures = 0
    while True:
        delivered = False
        connected_at = 0.0
        try:
            with transport.open_stream(
                op, _params(repo, last, topics, heartbeat_ms), run_id=run_id, idle_timeout_s=idle_s
            ) as response:
                connected_at = time.monotonic()
                parser = SseParser()
                for chunk in response.iter_bytes():
                    for frame in parser.feed(chunk):
                        event = _event(frame)
                        if event.seq <= last:
                            continue  # already delivered
                        last = event.seq
                        delivered = True
                        failures = 0
                        yield event
        except AgentGitError as err:
            if not err.retryable:
                raise
        except (httpx.TimeoutException, httpx.TransportError):
            pass  # idle watchdog or a dropped connection
        if connected_at and time.monotonic() - connected_at > STABLE_CONNECTION_S:
            failures = 0
        if not delivered:
            failures += 1
        if failures > policy.max_attempts:
            raise UnavailableError(
                f"the event stream could not be re-established after {policy.max_attempts} attempts",
                retryable=True,
                operation=op.id,
            )
        transport.sleep(0 if delivered else _backoff(policy, failures, transport.config.random()))


async def subscribe_async(
    transport: AsyncTransport,
    *,
    repo: str,
    after_seq: int | None,
    topics: Sequence[str] | None,
    heartbeat_ms: int | None,
    idle_timeout_ms: int | None,
    reconnect: ReconnectPolicy | None,
    run_id: str | None,
) -> AsyncGenerator[Event, None]:
    op = OPERATIONS["events.subscribe"]
    policy = reconnect or ReconnectPolicy()
    idle_s = _idle_seconds(idle_timeout_ms)
    last = after_seq
    if last is None:
        probe = await transport.execute(
            OPERATIONS["events.replay"],
            {"repo": repo, "after_seq": _PAST_THE_END, "limit": 1},
            None,
            CallOptions(run_id=run_id),
        )
        last = int(probe.get("latest_seq", 0))
    failures = 0
    while True:
        delivered = False
        connected_at = 0.0
        try:
            async with transport.open_stream(
                op, _params(repo, last, topics, heartbeat_ms), run_id=run_id, idle_timeout_s=idle_s
            ) as response:
                connected_at = time.monotonic()
                parser = SseParser()
                async for chunk in response.aiter_bytes():
                    for frame in parser.feed(chunk):
                        event = _event(frame)
                        if event.seq <= last:
                            continue
                        last = event.seq
                        delivered = True
                        failures = 0
                        yield event
        except AgentGitError as err:
            if not err.retryable:
                raise
        except (httpx.TimeoutException, httpx.TransportError):
            pass
        if connected_at and time.monotonic() - connected_at > STABLE_CONNECTION_S:
            failures = 0
        if not delivered:
            failures += 1
        if failures > policy.max_attempts:
            raise UnavailableError(
                f"the event stream could not be re-established after {policy.max_attempts} attempts",
                retryable=True,
                operation=op.id,
            )
        await transport.sleep(0 if delivered else _backoff(policy, failures, transport.config.random()))
