"""What the generated namespace classes need from a client."""

from __future__ import annotations

from collections.abc import AsyncGenerator, Generator, Mapping, Sequence
from typing import Any, Protocol, TypeVar

from agentgit._generated.models import Event, PutObjectResult
from agentgit._options import CallOptions, ReconnectPolicy

T = TypeVar("T")


class SyncCaller(Protocol):
    def _call(
        self, op_id: str, params: Mapping[str, Any], options: CallOptions | None, result: type[T]
    ) -> T: ...

    def _call_put(
        self, *, repo: str, object_id: str, data: bytes, options: CallOptions | None
    ) -> PutObjectResult: ...

    def _subscribe(
        self,
        *,
        repo: str,
        after_seq: int | None,
        topics: Sequence[str] | None,
        heartbeat_ms: int | None,
        idle_timeout_ms: int | None,
        reconnect: ReconnectPolicy | None,
        run_id: str | None,
    ) -> Generator[Event, None, None]: ...


class AsyncCaller(Protocol):
    async def _call(
        self, op_id: str, params: Mapping[str, Any], options: CallOptions | None, result: type[T]
    ) -> T: ...

    async def _call_put(
        self, *, repo: str, object_id: str, data: bytes, options: CallOptions | None
    ) -> PutObjectResult: ...

    def _subscribe(
        self,
        *,
        repo: str,
        after_seq: int | None,
        topics: Sequence[str] | None,
        heartbeat_ms: int | None,
        idle_timeout_ms: int | None,
        reconnect: ReconnectPolicy | None,
        run_id: str | None,
    ) -> AsyncGenerator[Event, None]: ...
