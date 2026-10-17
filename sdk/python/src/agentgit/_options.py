"""Options shared by the synchronous and the asynchronous client."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class RetryPolicy:
    """Automatic retries: only for errors the server marks retryable, and only when repeating the
    request cannot apply it twice (see the README, "Errors, retries, idempotency and deadlines")."""

    max_attempts: int = 4
    """Total attempts including the first. ``1`` disables retries."""
    base_delay_ms: int = 200
    """First backoff window; doubles per attempt, with full jitter."""
    max_delay_ms: int = 10_000
    """Cap of the backoff window."""
    max_retry_after_ms: int = 30_000
    """A server-supplied ``retry_after_ms`` / ``Retry-After`` above this is not waited for."""
    retry_unsafe: bool = False
    """Also retry operations the server cannot deduplicate. They may then be applied twice."""

    @classmethod
    def disabled(cls) -> RetryPolicy:
        return cls(max_attempts=1)


@dataclass(frozen=True)
class ReconnectPolicy:
    """How an event subscription re-establishes a dropped stream."""

    max_attempts: float = float("inf")
    """Consecutive failed reconnects before giving up."""
    base_delay_ms: int = 200
    max_delay_ms: int = 10_000


@dataclass
class CallOptions:
    """Per-call settings; every field is optional."""

    timeout_ms: int | None = None
    """Per-attempt deadline: sent as ``Request-Timeout`` and enforced locally."""
    idempotency_key: str | None = None
    """Overrides the generated ``Idempotency-Key`` of a mutating call."""
    run_id: str | None = None
    """Act as this agent run (``ag-run`` header) for this call."""
    headers: Mapping[str, str] | None = None
    """Extra request headers (for example ``traceparent``)."""
    retry: RetryPolicy | Literal[False] | None = None
    """Per-call retry policy; ``False`` disables retries."""
    request_id: str | None = None
    """Overrides the correlation id (``X-Request-Id``)."""
