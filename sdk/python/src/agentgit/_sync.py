"""The synchronous transport: one logical call with retries, deadlines and error mapping."""

from __future__ import annotations

import time
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from typing import Any

import httpx

from agentgit._config import SyncTokenSource, TransportConfig
from agentgit._core import (
    build_request,
    decode_json_body,
    effective_retry_class,
    error_from_response,
    grace_ms,
    network_error,
    random_token,
    request_headers,
    resolve_retry,
    retry_delay_seconds,
    should_retry,
    timeout_error,
    timeout_ms_for,
)
from agentgit._operation import OperationInfo
from agentgit._options import CallOptions
from agentgit.errors import AgentGitError


class SyncTransport:
    def __init__(
        self,
        config: TransportConfig,
        http: httpx.Client,
        token: SyncTokenSource | None,
        sleep: Callable[[float], None],
    ) -> None:
        self.config = config
        self.http = http
        self._token = token
        self.sleep = sleep

    def token(self) -> str | None:
        t = self._token
        return t() if callable(t) else t

    def execute(
        self,
        op: OperationInfo,
        params: Mapping[str, Any],
        data: bytes | None,
        options: CallOptions | None,
    ) -> Any:
        """Performs a call with retries; returns the decoded JSON (``dict``) or the raw ``bytes``."""
        opts = options or CallOptions()
        built = build_request(op, params, self.config.base_url)
        explicit = params.get("idempotency_key") or opts.idempotency_key
        key = (
            (explicit if isinstance(explicit, str) and explicit else f"sdk-{random_token(24)}")
            if (built.has_json_body and op.idempotency_key) or op.retry == "keyed"
            else None
        )
        request_id = opts.request_id or f"sdk-{random_token(16)}"
        policy = resolve_retry(self.config.retry, opts.retry)
        retry_class = effective_retry_class(op, params)
        timeout_ms = timeout_ms_for(params, options, self.config.request_timeout_ms)
        attempt = 0
        while True:
            attempt += 1
            rid = request_id if attempt == 1 else f"{request_id}-r{attempt}"
            try:
                return self._attempt(op, built, data, opts, key, rid, timeout_ms)
            except AgentGitError as err:
                failure = err
            except httpx.TimeoutException:
                failure = timeout_error(op, timeout_ms, rid)
            except (httpx.TransportError, OSError) as exc:
                failure = network_error(exc, op)
            failure.attempts = attempt
            if not should_retry(failure, retry_class, policy, attempt):
                raise failure
            delay = retry_delay_seconds(failure, attempt, policy, self.config.random())
            if delay is None:
                raise failure
            self.sleep(delay)

    def _headers(
        self, op: OperationInfo, opts: CallOptions, request_id: str, timeout_ms: int | None, key: str | None
    ) -> dict[str, str]:
        return request_headers(
            op,
            request_id=request_id,
            token=self.token(),
            run_id=opts.run_id or self.config.run_id,
            timeout_ms=timeout_ms,
            idempotency_key=key,
            base=self.config.headers,
            extra=opts.headers,
        )

    def _attempt(
        self,
        op: OperationInfo,
        built: Any,
        data: bytes | None,
        opts: CallOptions,
        key: str | None,
        request_id: str,
        timeout_ms: int,
    ) -> Any:
        headers = self._headers(op, opts, request_id, timeout_ms, key)
        content: bytes | None = None
        if built.has_json_body:
            headers["content-type"] = "application/json"
            content = built.json_body
        elif op.body == "bytes":
            headers["content-type"] = "application/octet-stream"
            content = data
        headers["accept"] = (
            "application/octet-stream, application/json" if op.response == "bytes" else "application/json"
        )
        total = (timeout_ms + grace_ms(timeout_ms)) / 1000 if timeout_ms > 0 else None
        deadline = time.monotonic() + total if total is not None else None
        with self.http.stream(
            built.method, built.url, headers=headers, content=content, timeout=httpx.Timeout(total)
        ) as response:
            chunks: list[bytes] = []
            for chunk in response.iter_bytes():
                chunks.append(chunk)
                if deadline is not None and time.monotonic() > deadline:
                    raise timeout_error(op, timeout_ms, request_id)
            raw = b"".join(chunks)
            status = response.status_code
            response_headers = dict(response.headers)
        if status >= 400:
            raise error_from_response(
                status, raw.decode("utf-8", "replace"), response_headers, op, request_id
            )
        if op.response == "bytes":
            return raw
        return decode_json_body(raw.decode("utf-8", "replace"), op, status, request_id)

    @contextmanager
    def open_stream(
        self,
        op: OperationInfo,
        params: Mapping[str, Any],
        *,
        run_id: str | None,
        idle_timeout_s: float | None,
    ) -> Iterator[httpx.Response]:
        """Opens a streaming response (SSE); status errors are raised like in ``execute``."""
        built = build_request(op, params, self.config.base_url)
        request_id = f"sdk-{random_token(16)}"
        opts = CallOptions(run_id=run_id)
        headers = self._headers(op, opts, request_id, None, None)
        headers["accept"] = "text/event-stream"
        # the connect, write and pool phases follow the client's deadline per attempt (0 = no limit); only the read
        # side is the idle watchdog
        connect_s = self.config.request_timeout_ms / 1000 if self.config.request_timeout_ms > 0 else None
        timeout = httpx.Timeout(connect=connect_s, read=idle_timeout_s, write=connect_s, pool=connect_s)
        try:
            with self.http.stream("GET", built.url, headers=headers, timeout=timeout) as response:
                if response.status_code >= 400:
                    body = response.read().decode("utf-8", "replace")
                    raise error_from_response(
                        response.status_code, body, dict(response.headers), op, request_id
                    )
                yield response
        except httpx.TimeoutException:
            raise
        except (httpx.TransportError, OSError) as exc:
            raise network_error(exc, op) from exc
