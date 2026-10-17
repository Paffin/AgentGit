from __future__ import annotations

import asyncio

import httpx
import pytest

from agentgit import (
    AgentGitClient,
    AgentGitError,
    AlreadyExistsError,
    AsyncAgentGitClient,
    FailedPreconditionError,
    InvalidArgumentError,
    NotFoundError,
    PermissionDeniedError,
    UnauthenticatedError,
)
from tests.faults import FaultyTransport
from tests.harness import RunningServer


def test_negotiates_without_credentials_and_checks_the_protocol_version(server: RunningServer) -> None:
    with server.anonymous() as client:
        hello = client.negotiate(client_name="sdk-test", client_version="0.1.0")
    assert hello.protocol_version == "agwp.v1"
    assert "events.replay" in hello.features
    assert hello.limits.max_page_size > 0
    assert "bearer" in hello.auth_methods


def test_refuses_a_server_that_speaks_another_protocol_version(server: RunningServer) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"protocol_version": "agwp.v9", "features": []})

    with AgentGitClient(
        server.base_url, http_client=httpx.Client(transport=httpx.MockTransport(handler))
    ) as c:
        with pytest.raises(FailedPreconditionError):
            c.negotiate()


def test_whoami_reports_the_identity_behind_the_credential(server: RunningServer) -> None:
    with server.admin() as admin:
        me = admin.session.whoami()
        assert me.method == "bootstrap"
        assert "*:admin" in me.roles
    with server.actor("user:alice", "acme/none", "reader") as alice:
        who = alice.session.whoami()
        assert who.identity == "user:alice"
        assert who.method == "token"
        assert who.expires_at_ms > 0


def test_maps_the_error_envelope_onto_a_typed_hierarchy(server: RunningServer) -> None:
    with AgentGitClient(server.base_url, "agt_0000000000000000_nope", retry=False) as bad:
        with pytest.raises(UnauthenticatedError) as info:
            bad.session.whoami()
    e = info.value
    assert isinstance(e, AgentGitError)
    assert (e.code, e.status, e.retryable) == ("unauthenticated", 401, False)
    assert e.request_id.startswith("sdk-")  # the id the client sent comes back
    assert e.operation == "session.whoami"
    assert e.is_code("unauthenticated", "permission_denied")
    assert "unauthenticated" in str(e)


def test_precise_codes_for_not_found_already_exists_and_invalid_argument(server: RunningServer) -> None:
    with server.admin(retry=False) as admin:
        admin.repos.create(name="acme/errs")
        with pytest.raises(AlreadyExistsError) as exists:
            admin.repos.create(name="acme/errs")
        assert exists.value.status == 409
        with pytest.raises(NotFoundError) as missing:
            admin.repos.describe(repo="acme/missing")
        assert missing.value.status == 404
        with pytest.raises(InvalidArgumentError):
            admin.repos.create(name="Not A Valid Name!")


def test_repositories_are_hidden_from_callers_without_a_role(server: RunningServer) -> None:
    with server.admin() as admin:
        admin.repos.create(name="acme/secret")
    with server.actor("user:mallory", "other/*", "writer", retry=False) as mallory:
        with pytest.raises(NotFoundError):
            mallory.repos.describe(repo="acme/secret")
        with pytest.raises(PermissionDeniedError):
            mallory.repos.create(name="acme/mine")


def test_an_answer_that_is_not_the_json_envelope_becomes_a_typed_error(server: RunningServer) -> None:
    def to_delete(request: httpx.Request) -> httpx.Response:
        # The server answers 405 with an empty body for a verb a route does not support.
        return httpx.Client().send(httpx.Request("DELETE", request.url, headers=request.headers))

    with server.admin(
        retry=False, http_client=httpx.Client(transport=httpx.MockTransport(to_delete))
    ) as client:
        with pytest.raises(AgentGitError) as info:
            client.repos.describe(repo="acme/errs")
    assert (info.value.status, info.value.code) == (405, "http_error")


def test_fails_fast_before_sending_anything(server: RunningServer) -> None:
    transport = FaultyTransport()
    with server.admin(http_client=httpx.Client(transport=transport)) as admin:
        with pytest.raises(ValueError, match=r"owner/name"):
            admin.repos.describe(repo="no-slash")
        with pytest.raises(ValueError, match=r'missing required field "id"'):
            admin.proposals.get(repo="acme/errs", id="")
        with pytest.raises(TypeError):
            admin.proposals.get(repo="acme/errs")  # type: ignore[call-arg]
    assert transport.seen == []


def test_sends_the_correlation_id_deadline_and_extra_headers(server: RunningServer) -> None:
    transport = FaultyTransport()
    http = httpx.Client(transport=transport)
    with server.admin(
        http_client=http,
        request_timeout_ms=12_345,
        headers={"traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"},
    ) as client:
        from agentgit import CallOptions

        client.repos.describe(repo="acme/errs", options=CallOptions(request_id="my-correlation-id"))
        h = transport.seen[0].headers
        assert h["authorization"] == f"Bearer {server.bootstrap_token}"
        assert h["x-request-id"] == "my-correlation-id"
        assert h["request-timeout"] == "12345"
        assert h["traceparent"].startswith("00-")
        assert "idempotency-key" not in h  # reads carry none
        transport.reset()
        client.session.negotiate()
        assert "authorization" not in transport.seen[0].headers  # public route: no credentials sent


def test_the_async_client_maps_errors_the_same_way(server: RunningServer) -> None:
    async def scenario() -> None:
        async with AsyncAgentGitClient(server.base_url, "agt_0000000000000000_nope", retry=False) as bad:
            with pytest.raises(UnauthenticatedError):
                await bad.session.whoami()
        async with server.async_admin() as admin:
            hello = await admin.negotiate(client_name="async-test")
            assert hello.protocol_version == "agwp.v1"
            with pytest.raises(NotFoundError):
                await admin.repos.describe(repo="acme/missing")

    asyncio.run(scenario())
