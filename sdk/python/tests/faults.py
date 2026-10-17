"""``httpx`` transports that wrap the real network, record every request and can inject faults."""

from __future__ import annotations

from collections.abc import AsyncIterator, Callable, Iterator
from dataclasses import dataclass, field

import httpx


@dataclass
class Seen:
    method: str
    url: str
    path: str  # path + query, as sent
    headers: httpx.Headers
    body: bytes


@dataclass
class Reply:
    """Answer from the transport itself, without reaching the server."""

    status: int
    headers: dict[str, str] = field(default_factory=dict)
    body: str = ""


Fault = str | Reply  # "pass" | "drop-response" | "refuse" | Reply


def _seen(request: httpx.Request) -> Seen:
    return Seen(
        request.method,
        str(request.url),
        request.url.raw_path.decode("ascii"),
        request.headers,
        request.content,
    )


class _Plan:
    def __init__(self) -> None:
        self.seen: list[Seen] = []
        self.rule: Callable[[Seen, int], Fault] = lambda _req, _n: "pass"
        self.match: Callable[[Seen], bool] = lambda _req: True
        self._n = 0

    def reset(self) -> None:
        self.seen.clear()
        self._n = 0

    def decide(self, request: httpx.Request) -> tuple[Seen, Fault]:
        request.read()
        seen = _seen(request)
        self.seen.append(seen)
        fault: Fault = "pass"
        if self.match(seen):
            self._n += 1
            fault = self.rule(seen, self._n)
        return seen, fault

    def to(self, prefix: str, method: str | None = None) -> list[Seen]:
        return [s for s in self.seen if s.path.startswith(prefix) and (method is None or s.method == method)]


class FaultyTransport(httpx.BaseTransport, _Plan):
    """Synchronous."""

    def __init__(self) -> None:
        _Plan.__init__(self)
        self._real = httpx.HTTPTransport()

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        _seen_req, fault = self.decide(request)
        if fault == "refuse":
            raise httpx.ConnectError("connection refused (injected)")
        if isinstance(fault, Reply):
            return httpx.Response(fault.status, headers=fault.headers, content=fault.body.encode())
        response = self._real.handle_request(request)
        if fault == "drop-response":
            response.read()
            response.close()
            raise httpx.ReadError("socket hang up (injected)")
        return response

    def close(self) -> None:
        self._real.close()


class AsyncFaultyTransport(httpx.AsyncBaseTransport, _Plan):
    """Asynchronous."""

    def __init__(self) -> None:
        _Plan.__init__(self)
        self._real = httpx.AsyncHTTPTransport()

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        _seen_req, fault = self.decide(request)
        if fault == "refuse":
            raise httpx.ConnectError("connection refused (injected)")
        if isinstance(fault, Reply):
            return httpx.Response(fault.status, headers=fault.headers, content=fault.body.encode())
        response = await self._real.handle_async_request(request)
        if fault == "drop-response":
            await response.aread()
            await response.aclose()
            raise httpx.ReadError("socket hang up (injected)")
        return response

    async def aclose(self) -> None:
        await self._real.aclose()


class RecordedSleep:
    """Makes sleeping instantaneous and records the requested delays (in seconds)."""

    def __init__(self) -> None:
        self.delays: list[float] = []

    def __call__(self, seconds: float) -> None:
        self.delays.append(seconds)

    async def asleep(self, seconds: float) -> None:
        self.delays.append(seconds)


class BreakingStream(httpx.SyncByteStream):
    """Forwards an event stream frame by frame and breaks it after ``after`` events."""

    def __init__(self, upstream: httpx.SyncByteStream, after: int, how: str) -> None:
        self._upstream = upstream
        self._after = after
        self._how = how

    def __iter__(self) -> Iterator[bytes]:
        buffer = b""
        events = 0
        for chunk in self._upstream:
            buffer += chunk
            while b"\n\n" in buffer:
                frame, buffer = buffer.split(b"\n\n", 1)
                yield frame + b"\n\n"
                if frame.startswith(b"id:"):
                    events += 1
                if events >= self._after:
                    self._upstream.close()
                    if self._how == "error":
                        raise httpx.ReadError("terminated (injected)")
                    return

    def close(self) -> None:
        self._upstream.close()


class AsyncBreakingStream(httpx.AsyncByteStream):
    def __init__(self, upstream: httpx.AsyncByteStream, after: int, how: str) -> None:
        self._upstream = upstream
        self._after = after
        self._how = how

    async def __aiter__(self) -> AsyncIterator[bytes]:
        buffer = b""
        events = 0
        async for chunk in self._upstream:
            buffer += chunk
            while b"\n\n" in buffer:
                frame, buffer = buffer.split(b"\n\n", 1)
                yield frame + b"\n\n"
                if frame.startswith(b"id:"):
                    events += 1
                if events >= self._after:
                    await self._upstream.aclose()
                    if self._how == "error":
                        raise httpx.ReadError("terminated (injected)")
                    return

    async def aclose(self) -> None:
        await self._upstream.aclose()


class BreakingTransport(httpx.BaseTransport):
    """Breaks the n-th event-stream connection after ``plan[n] = (events, 'close' | 'error')``."""

    def __init__(self, plan: list[tuple[int, str]]) -> None:
        self._real = httpx.HTTPTransport()
        self._plan = plan
        self.urls: list[str] = []
        self._connection = 0

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        response = self._real.handle_request(request)
        if "/events?" not in str(request.url):
            return response
        self.urls.append(str(request.url))
        if self._connection >= len(self._plan):
            return response
        after, how = self._plan[self._connection]
        self._connection += 1
        assert isinstance(response.stream, httpx.SyncByteStream)
        return httpx.Response(
            response.status_code, headers=response.headers, stream=BreakingStream(response.stream, after, how)
        )

    def close(self) -> None:
        self._real.close()


class AsyncBreakingTransport(httpx.AsyncBaseTransport):
    def __init__(self, plan: list[tuple[int, str]]) -> None:
        self._real = httpx.AsyncHTTPTransport()
        self._plan = plan
        self.urls: list[str] = []
        self._connection = 0

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        response = await self._real.handle_async_request(request)
        if "/events?" not in str(request.url):
            return response
        self.urls.append(str(request.url))
        if self._connection >= len(self._plan):
            return response
        after, how = self._plan[self._connection]
        self._connection += 1
        assert isinstance(response.stream, httpx.AsyncByteStream)
        return httpx.Response(
            response.status_code,
            headers=response.headers,
            stream=AsyncBreakingStream(response.stream, after, how),
        )

    async def aclose(self) -> None:
        await self._real.aclose()
