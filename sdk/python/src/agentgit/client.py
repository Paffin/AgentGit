"""The clients: one namespace per area of the API, one method per OpenAPI operation id."""

from __future__ import annotations

import random as _random
import time
from collections.abc import AsyncGenerator, AsyncIterator, Callable, Generator, Iterator, Mapping, Sequence
from contextlib import aclosing
from dataclasses import dataclass, field
from types import TracebackType
from typing import Any, Literal, TypeVar, overload

import httpx

from agentgit._async import AsyncTransport
from agentgit._codec import decode_as
from agentgit._config import AsyncTokenSource, SyncTokenSource, TransportConfig
from agentgit._core import DEFAULT_TIMEOUT_MS
from agentgit._events import subscribe_async, subscribe_sync
from agentgit._generated.models import (
    Changeset,
    DirEntry,
    Event,
    NegotiateResponse,
    Proposal,
    PutObjectResult,
    Repo,
    SearchMatch,
    TreeEntry,
)
from agentgit._generated.namespaces import (
    AdminApi,
    AgentsApi,
    AsyncAdminApi,
    AsyncAgentsApi,
    AsyncCredentialsApi,
    AsyncEventsApi,
    AsyncHistoryApi,
    AsyncIntegrationApi,
    AsyncJobsApi,
    AsyncLeasesApi,
    AsyncMigrationsApi,
    AsyncObjectsApi,
    AsyncOperationsApi,
    AsyncProposalsApi,
    AsyncProvenanceApi,
    AsyncQueueApi,
    AsyncReposApi,
    AsyncRolesApi,
    AsyncSessionApi,
    AsyncStreamsApi,
    AsyncTasksApi,
    AsyncTreeApi,
    AsyncWorkspacesApi,
    CredentialsApi,
    EventsApi,
    HistoryApi,
    IntegrationApi,
    JobsApi,
    LeasesApi,
    MigrationsApi,
    ObjectsApi,
    OperationsApi,
    ProposalsApi,
    ProvenanceApi,
    QueueApi,
    ReposApi,
    RolesApi,
    SessionApi,
    StreamsApi,
    TasksApi,
    TreeApi,
    WorkspacesApi,
)
from agentgit._generated.operations import OPERATIONS, PAGINATED
from agentgit._options import CallOptions, ReconnectPolicy, RetryPolicy
from agentgit._sync import SyncTransport
from agentgit.errors import FailedPreconditionError
from agentgit.hashing import StoredObject, blob_object

PROTOCOL_VERSION = "agwp.v1"
"""The protocol version this client speaks."""

T = TypeVar("T")


@dataclass
class PutBlobResult(PutObjectResult):
    """The answer of ``objects.put_blob``."""

    id: str = ""
    """The content id of the blob."""


@dataclass
class UploadResult:
    """The answer of ``objects.upload``."""

    stored: list[str] = field(default_factory=list)
    """Ids that were sent."""
    already_present: list[str] = field(default_factory=list)
    """Ids the repository already had."""


def _binary_body(op_id: str, params: Mapping[str, Any]) -> tuple[Mapping[str, Any], bytes | None]:
    if OPERATIONS[op_id].body != "bytes":
        return params, None
    fields = dict(params)
    data = fields.pop("data", None)
    if not isinstance(data, (bytes, bytearray, memoryview)):
        raise TypeError("binary operations require bytes in data")
    return fields, bytes(data)


def _retry_policy(retry: RetryPolicy | bool | None) -> RetryPolicy:
    if retry is False:
        return RetryPolicy.disabled()
    if isinstance(retry, RetryPolicy):
        return retry
    return RetryPolicy()


def _config(
    base_url: str,
    run_id: str | None,
    request_timeout_ms: int,
    retry: RetryPolicy | bool | None,
    headers: Mapping[str, str] | None,
    rand: Callable[[], float] | None,
) -> TransportConfig:
    if not base_url:
        raise ValueError("base_url is required")
    return TransportConfig(
        base_url=base_url.rstrip("/"),
        run_id=run_id,
        request_timeout_ms=request_timeout_ms,
        retry=_retry_policy(retry),
        headers=dict(headers or {}),
        random=rand or _random.random,
    )


def _negotiated(hello: NegotiateResponse) -> NegotiateResponse:
    if hello.protocol_version != PROTOCOL_VERSION:
        raise FailedPreconditionError(
            f"the server speaks {hello.protocol_version or 'an unknown protocol'}, this client speaks {PROTOCOL_VERSION}",
            operation="session.negotiate",
        )
    return hello


def _negotiate_params(client_name: str | None, client_version: str | None) -> dict[str, Any]:
    # The versions this client speaks travel as a repeated query parameter; a server that speaks none of
    # them answers failed_precondition.
    return {
        "supported_versions": [PROTOCOL_VERSION],
        "client_name": client_name,
        "client_version": client_version,
    }


class _ObjectsExt(ObjectsApi):
    """The generated ``objects`` operations plus two helpers."""

    __slots__ = ()

    def put_blob(
        self, *, repo: str, content: bytes | str, options: CallOptions | None = None
    ) -> PutBlobResult:
        """Uploads a blob (computing its id and envelope) and returns the id."""
        stored = blob_object(content)
        result = self.put(repo=repo, object_id=stored.id, data=stored.envelope, options=options)
        return PutBlobResult(object_id=result.object_id, stored=result.stored, id=stored.id)

    def upload(
        self, *, repo: str, objects: Sequence[StoredObject], options: CallOptions | None = None
    ) -> UploadResult:
        """Uploads the objects the repository lacks (``objects.missing``, then ``objects.put``), in the
        order given - blobs before the changeset that uses them. Pass ``built.objects()``."""
        by_id = {o.id: o for o in objects}
        ids = list(by_id)
        missing: set[str] = set()
        for i in range(0, len(ids), 1000):
            answer = self.missing(repo=repo, object_ids=ids[i : i + 1000], options=options)
            missing.update(answer.missing)
        result = UploadResult()
        for oid in ids:
            if oid in missing:
                self.put(repo=repo, object_id=oid, data=by_id[oid].envelope, options=options)
                result.stored.append(oid)
            else:
                result.already_present.append(oid)
        return result


class _AsyncObjectsExt(AsyncObjectsApi):
    __slots__ = ()

    async def put_blob(
        self, *, repo: str, content: bytes | str, options: CallOptions | None = None
    ) -> PutBlobResult:
        """Uploads a blob (computing its id and envelope) and returns the id."""
        stored = blob_object(content)
        result = await self.put(repo=repo, object_id=stored.id, data=stored.envelope, options=options)
        return PutBlobResult(object_id=result.object_id, stored=result.stored, id=stored.id)

    async def upload(
        self, *, repo: str, objects: Sequence[StoredObject], options: CallOptions | None = None
    ) -> UploadResult:
        """Uploads the objects the repository lacks, in the order given."""
        by_id = {o.id: o for o in objects}
        ids = list(by_id)
        missing: set[str] = set()
        for i in range(0, len(ids), 1000):
            answer = await self.missing(repo=repo, object_ids=ids[i : i + 1000], options=options)
            missing.update(answer.missing)
        result = UploadResult()
        for oid in ids:
            if oid in missing:
                await self.put(repo=repo, object_id=oid, data=by_id[oid].envelope, options=options)
                result.stored.append(oid)
            else:
                result.already_present.append(oid)
        return result


class AgentGitClient:
    """A synchronous client.

    Args:
        base_url: server root, for example ``https://git.example.com`` (a path prefix behind a proxy is kept).
        token: a bearer credential (``agt_...``, an OIDC token or the bootstrap token), or a function
            returning a fresh one for each request.
        run_id: act as this agent run (``ag-run``): requests are then attributed to ``agent:<name>#<run>``.
        request_timeout_ms: deadline per attempt, sent as ``Request-Timeout`` and enforced locally
            (default 30 000; ``0`` = none).
        retry: a :class:`RetryPolicy`, or ``False`` to disable retries.
        headers: headers added to every request.
        http_client: your own ``httpx.Client`` (proxies, TLS, a test transport). It is not closed by
            :meth:`close`.
        sleep: replaces ``time.sleep`` between retries (tests).
        random: replaces the jitter source, a function returning a float in [0, 1) (tests).
    """

    session: SessionApi
    repos: ReposApi
    workspaces: WorkspacesApi
    proposals: ProposalsApi
    integration: IntegrationApi
    queue: QueueApi
    streams: StreamsApi
    history: HistoryApi
    tree: TreeApi
    objects: _ObjectsExt
    operations: OperationsApi
    agents: AgentsApi
    tasks: TasksApi
    leases: LeasesApi
    provenance: ProvenanceApi
    events: EventsApi
    jobs: JobsApi
    migrations: MigrationsApi
    admin: AdminApi
    credentials: CredentialsApi
    roles: RolesApi

    def __init__(
        self,
        base_url: str,
        token: SyncTokenSource | None = None,
        *,
        run_id: str | None = None,
        request_timeout_ms: int = DEFAULT_TIMEOUT_MS,
        retry: RetryPolicy | bool | None = None,
        headers: Mapping[str, str] | None = None,
        http_client: httpx.Client | None = None,
        sleep: Callable[[float], None] | None = None,
        random: Callable[[], float] | None = None,
    ) -> None:
        self._init_args: dict[str, Any] = {
            "base_url": base_url,
            "run_id": run_id,
            "request_timeout_ms": request_timeout_ms,
            "retry": retry,
            "headers": headers,
            "sleep": sleep,
            "random": random,
        }
        self._http = http_client or httpx.Client()
        self._owns_http = http_client is None
        self._token = token
        self._transport = SyncTransport(
            _config(base_url, run_id, request_timeout_ms, retry, headers, random),
            self._http,
            token,
            sleep or time.sleep,
        )
        self.session = SessionApi(self)
        self.repos = ReposApi(self)
        self.workspaces = WorkspacesApi(self)
        self.proposals = ProposalsApi(self)
        self.integration = IntegrationApi(self)
        self.queue = QueueApi(self)
        self.streams = StreamsApi(self)
        self.history = HistoryApi(self)
        self.tree = TreeApi(self)
        self.objects = _ObjectsExt(self)
        self.operations = OperationsApi(self)
        self.agents = AgentsApi(self)
        self.tasks = TasksApi(self)
        self.leases = LeasesApi(self)
        self.provenance = ProvenanceApi(self)
        self.events = EventsApi(self)
        self.jobs = JobsApi(self)
        self.migrations = MigrationsApi(self)
        self.admin = AdminApi(self)
        self.credentials = CredentialsApi(self)
        self.roles = RolesApi(self)

    # -- lifecycle ----------------------------------------------------------------------------

    @property
    def base_url(self) -> str:
        return self._transport.config.base_url

    def close(self) -> None:
        """Closes the HTTP connection pool, unless the ``httpx.Client`` was passed in."""
        if self._owns_http:
            self._http.close()

    def __enter__(self) -> AgentGitClient:
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        self.close()

    def _derive(self, **overrides: Any) -> AgentGitClient:
        token = overrides.pop("token", self._token)
        args = {**self._init_args, **overrides}
        return AgentGitClient(args.pop("base_url"), token, http_client=self._http, **args)

    def with_run(self, run_id: str) -> AgentGitClient:
        """A client that acts as the given agent run (every request carries ``ag-run``). Shares the connection pool."""
        return self._derive(run_id=run_id)

    def with_token(self, token: SyncTokenSource) -> AgentGitClient:
        """A client with another credential, everything else equal. Shares the connection pool."""
        return self._derive(token=token)

    # -- what the generated namespaces call -----------------------------------------------------

    def _call(self, op_id: str, params: Mapping[str, Any], options: CallOptions | None, result: type[T]) -> T:
        params, data = _binary_body(op_id, params)
        raw = self._transport.execute(OPERATIONS[op_id], params, data, options)
        if result is bytes:
            return raw  # type: ignore[no-any-return]
        return decode_as(result, raw)

    def _call_put(
        self, *, repo: str, object_id: str, data: bytes, options: CallOptions | None
    ) -> PutObjectResult:
        raw = self._transport.execute(
            OPERATIONS["objects.put"], {"repo": repo, "object_id": object_id}, bytes(data), options
        )
        return decode_as(PutObjectResult, raw)

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
    ) -> Generator[Event, None, None]:
        return subscribe_sync(
            self._transport,
            repo=repo,
            after_seq=after_seq,
            topics=topics,
            heartbeat_ms=heartbeat_ms,
            idle_timeout_ms=idle_timeout_ms,
            reconnect=reconnect,
            run_id=run_id or self._transport.config.run_id,
        )

    # -- helpers --------------------------------------------------------------------------------

    def call(self, op_id: str, /, *, options: CallOptions | None = None, **params: Any) -> Any:
        """Calls any JSON operation by its OpenAPI operation id; the answer is the decoded model.

        The namespaced methods (``client.proposals.create(...)``) are the same thing with nicer names.
        """
        op = OPERATIONS[op_id]
        method = getattr(getattr(self, op.namespace), op.name)
        return method(**params, options=options)

    def negotiate(
        self,
        *,
        client_name: str | None = None,
        client_version: str | None = None,
        options: CallOptions | None = None,
    ) -> NegotiateResponse:
        """Asks the server what it supports and checks that it speaks ``agwp.v1``.

        Raises :class:`FailedPreconditionError` otherwise.
        """
        hello = self._call(
            "session.negotiate", _negotiate_params(client_name, client_version), options, NegotiateResponse
        )
        return _negotiated(hello)

    @overload
    def paginate(
        self,
        op_id: Literal["repo.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[Repo]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["history.log"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[Changeset]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["history.ancestors"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[str]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["proposal.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[Proposal]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.get", "jobs.candidate"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[TreeEntry]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[DirEntry]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.search"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> Iterator[SearchMatch]: ...
    def paginate(
        self,
        op_id: str,
        /,
        *,
        options: CallOptions | None = None,
        max_items: int | None = None,
        **params: Any,
    ) -> Iterator[Any]:
        """Iterates over every item of a paginated listing, following ``next_page_token``::

        for proposal in client.paginate("proposal.list", repo=repo, state="open"): ...
        """
        key = PAGINATED[op_id]
        seen = 0
        for page in self.pages(op_id, options=options, **params):
            for item in getattr(page, key):
                if max_items is not None and seen >= max_items:
                    return
                seen += 1
                yield item

    def pages(self, op_id: str, /, *, options: CallOptions | None = None, **params: Any) -> Iterator[Any]:
        """Like :meth:`paginate`, but yields whole response pages."""
        if op_id not in PAGINATED:
            raise ValueError(f"{op_id} is not a paginated operation")
        token = params.pop("page_token", None) or ""
        while True:
            page = self.call(op_id, options=options, page_token=token, **params)
            yield page
            nxt = page.next_page_token
            if not nxt or nxt == token:
                return
            token = nxt

    def subscribe_events(
        self,
        *,
        repo: str,
        after_seq: int | None = None,
        topics: Sequence[str] | None = None,
        heartbeat_ms: int | None = None,
        idle_timeout_ms: int | None = None,
        reconnect: ReconnectPolicy | None = None,
        run_id: str | None = None,
    ) -> Generator[Event, None, None]:
        """Follows a repository's event feed over Server-Sent Events, resuming after the last delivered ``seq``."""
        return self.events.subscribe(
            repo=repo,
            after_seq=after_seq,
            topics=topics,
            heartbeat_ms=heartbeat_ms,
            idle_timeout_ms=idle_timeout_ms,
            reconnect=reconnect,
            run_id=run_id,
        )


class AsyncAgentGitClient:
    """The asynchronous client: the same surface as :class:`AgentGitClient`, with ``await`` and ``async for``.

    Use it as ``async with AsyncAgentGitClient(...) as client:`` (or call :meth:`aclose`).
    """

    session: AsyncSessionApi
    repos: AsyncReposApi
    workspaces: AsyncWorkspacesApi
    proposals: AsyncProposalsApi
    integration: AsyncIntegrationApi
    queue: AsyncQueueApi
    streams: AsyncStreamsApi
    history: AsyncHistoryApi
    tree: AsyncTreeApi
    objects: _AsyncObjectsExt
    operations: AsyncOperationsApi
    agents: AsyncAgentsApi
    tasks: AsyncTasksApi
    leases: AsyncLeasesApi
    provenance: AsyncProvenanceApi
    events: AsyncEventsApi
    jobs: AsyncJobsApi
    migrations: AsyncMigrationsApi
    admin: AsyncAdminApi
    credentials: AsyncCredentialsApi
    roles: AsyncRolesApi

    def __init__(
        self,
        base_url: str,
        token: AsyncTokenSource | None = None,
        *,
        run_id: str | None = None,
        request_timeout_ms: int = DEFAULT_TIMEOUT_MS,
        retry: RetryPolicy | bool | None = None,
        headers: Mapping[str, str] | None = None,
        http_client: httpx.AsyncClient | None = None,
        sleep: Callable[[float], Any] | None = None,
        random: Callable[[], float] | None = None,
    ) -> None:
        import asyncio

        self._init_args: dict[str, Any] = {
            "base_url": base_url,
            "run_id": run_id,
            "request_timeout_ms": request_timeout_ms,
            "retry": retry,
            "headers": headers,
            "sleep": sleep,
            "random": random,
        }
        self._http = http_client or httpx.AsyncClient()
        self._owns_http = http_client is None
        self._token = token
        self._transport = AsyncTransport(
            _config(base_url, run_id, request_timeout_ms, retry, headers, random),
            self._http,
            token,
            sleep or asyncio.sleep,
        )
        self.session = AsyncSessionApi(self)
        self.repos = AsyncReposApi(self)
        self.workspaces = AsyncWorkspacesApi(self)
        self.proposals = AsyncProposalsApi(self)
        self.integration = AsyncIntegrationApi(self)
        self.queue = AsyncQueueApi(self)
        self.streams = AsyncStreamsApi(self)
        self.history = AsyncHistoryApi(self)
        self.tree = AsyncTreeApi(self)
        self.objects = _AsyncObjectsExt(self)
        self.operations = AsyncOperationsApi(self)
        self.agents = AsyncAgentsApi(self)
        self.tasks = AsyncTasksApi(self)
        self.leases = AsyncLeasesApi(self)
        self.provenance = AsyncProvenanceApi(self)
        self.events = AsyncEventsApi(self)
        self.jobs = AsyncJobsApi(self)
        self.migrations = AsyncMigrationsApi(self)
        self.admin = AsyncAdminApi(self)
        self.credentials = AsyncCredentialsApi(self)
        self.roles = AsyncRolesApi(self)

    @property
    def base_url(self) -> str:
        return self._transport.config.base_url

    async def aclose(self) -> None:
        """Closes the HTTP connection pool, unless the ``httpx.AsyncClient`` was passed in."""
        if self._owns_http:
            await self._http.aclose()

    async def __aenter__(self) -> AsyncAgentGitClient:
        return self

    async def __aexit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        await self.aclose()

    def _derive(self, **overrides: Any) -> AsyncAgentGitClient:
        token = overrides.pop("token", self._token)
        args = {**self._init_args, **overrides}
        return AsyncAgentGitClient(args.pop("base_url"), token, http_client=self._http, **args)

    def with_run(self, run_id: str) -> AsyncAgentGitClient:
        """A client that acts as the given agent run. Shares the connection pool."""
        return self._derive(run_id=run_id)

    def with_token(self, token: AsyncTokenSource) -> AsyncAgentGitClient:
        """A client with another credential, everything else equal. Shares the connection pool."""
        return self._derive(token=token)

    async def _call(
        self, op_id: str, params: Mapping[str, Any], options: CallOptions | None, result: type[T]
    ) -> T:
        params, data = _binary_body(op_id, params)
        raw = await self._transport.execute(OPERATIONS[op_id], params, data, options)
        if result is bytes:
            return raw  # type: ignore[no-any-return]
        return decode_as(result, raw)

    async def _call_put(
        self, *, repo: str, object_id: str, data: bytes, options: CallOptions | None
    ) -> PutObjectResult:
        raw = await self._transport.execute(
            OPERATIONS["objects.put"], {"repo": repo, "object_id": object_id}, bytes(data), options
        )
        return decode_as(PutObjectResult, raw)

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
    ) -> AsyncGenerator[Event, None]:
        return subscribe_async(
            self._transport,
            repo=repo,
            after_seq=after_seq,
            topics=topics,
            heartbeat_ms=heartbeat_ms,
            idle_timeout_ms=idle_timeout_ms,
            reconnect=reconnect,
            run_id=run_id or self._transport.config.run_id,
        )

    async def call(self, op_id: str, /, *, options: CallOptions | None = None, **params: Any) -> Any:
        """Calls any JSON operation by its OpenAPI operation id; the answer is the decoded model."""
        op = OPERATIONS[op_id]
        method = getattr(getattr(self, op.namespace), op.name)
        return await method(**params, options=options)

    async def negotiate(
        self,
        *,
        client_name: str | None = None,
        client_version: str | None = None,
        options: CallOptions | None = None,
    ) -> NegotiateResponse:
        """Asks the server what it supports and checks that it speaks ``agwp.v1``."""
        hello = await self._call(
            "session.negotiate", _negotiate_params(client_name, client_version), options, NegotiateResponse
        )
        return _negotiated(hello)

    @overload
    def paginate(
        self,
        op_id: Literal["repo.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[Repo]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["history.log"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[Changeset]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["history.ancestors"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[str]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["proposal.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[Proposal]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.get", "jobs.candidate"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[TreeEntry]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.list"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[DirEntry]: ...
    @overload
    def paginate(
        self,
        op_id: Literal["tree.search"],
        /,
        *,
        options: CallOptions | None = ...,
        max_items: int | None = ...,
        **params: Any,
    ) -> AsyncIterator[SearchMatch]: ...
    def paginate(
        self,
        op_id: str,
        /,
        *,
        options: CallOptions | None = None,
        max_items: int | None = None,
        **params: Any,
    ) -> AsyncIterator[Any]:
        """Iterates over every item of a paginated listing, following ``next_page_token``::

        async for proposal in client.paginate("proposal.list", repo=repo, state="open"): ...
        """
        return self._paginate(op_id, options, max_items, params)

    async def _paginate(
        self, op_id: str, options: CallOptions | None, max_items: int | None, params: dict[str, Any]
    ) -> AsyncIterator[Any]:
        key = PAGINATED[op_id]
        seen = 0
        # aclosing: stopping early must also close the page generator, not leave it to the garbage collector.
        async with aclosing(self.pages(op_id, options=options, **params)) as pages:
            async for page in pages:
                for item in getattr(page, key):
                    if max_items is not None and seen >= max_items:
                        return
                    seen += 1
                    yield item

    async def pages(
        self, op_id: str, /, *, options: CallOptions | None = None, **params: Any
    ) -> AsyncGenerator[Any, None]:
        """Like :meth:`paginate`, but yields whole response pages."""
        if op_id not in PAGINATED:
            raise ValueError(f"{op_id} is not a paginated operation")
        token = params.pop("page_token", None) or ""
        while True:
            page = await self.call(op_id, options=options, page_token=token, **params)
            yield page
            nxt = page.next_page_token
            if not nxt or nxt == token:
                return
            token = nxt

    def subscribe_events(
        self,
        *,
        repo: str,
        after_seq: int | None = None,
        topics: Sequence[str] | None = None,
        heartbeat_ms: int | None = None,
        idle_timeout_ms: int | None = None,
        reconnect: ReconnectPolicy | None = None,
        run_id: str | None = None,
    ) -> AsyncGenerator[Event, None]:
        """Follows a repository's event feed over Server-Sent Events, resuming after the last delivered ``seq``."""
        return self.events.subscribe(
            repo=repo,
            after_seq=after_seq,
            topics=topics,
            heartbeat_ms=heartbeat_ms,
            idle_timeout_ms=idle_timeout_ms,
            reconnect=reconnect,
            run_id=run_id,
        )
