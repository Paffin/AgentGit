from __future__ import annotations

from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass

from agentgit._options import RetryPolicy

SyncTokenSource = str | Callable[[], str]
AsyncTokenSource = str | Callable[[], str | Awaitable[str]]


@dataclass
class TransportConfig:
    base_url: str
    run_id: str | None
    request_timeout_ms: int
    retry: RetryPolicy
    headers: Mapping[str, str]
    random: Callable[[], float]
