import asyncio
import json

import httpx

from agentgit import AgentGitClient, AsyncAgentGitClient

DRAFT = {
    "present": True,
    "version": 1,
    "head": "reviewed",
    "decision": "comment",
    "body": "Private notes",
    "updated_at_ms": 1,
}


def test_private_drafts_send_owner_free_guarded_requests() -> None:
    requests: list[httpx.Request] = []

    def respond(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        return httpx.Response(200, json=DRAFT)

    with AgentGitClient(
        "http://draft.test", "reviewer", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        client.proposals.get_draft(repo="team/app", id="proposal")
        client.proposals.save_draft(
            repo="team/app",
            id="proposal",
            expected_version=0,
            head="reviewed",
            decision="comment",
            body="Private notes",
        )
        client.proposals.clear_draft(repo="team/app", id="proposal", expected_version=1)
    assert [request.method for request in requests] == ["GET", "PUT", "DELETE"]
    assert all(request.url.path.endswith("/proposals/proposal/review-draft") for request in requests)
    assert json.loads(requests[1].content) == {
        "expected_version": 0,
        "head": "reviewed",
        "decision": "comment",
        "body": "Private notes",
    }
    assert requests[2].content == b""
    assert requests[2].url.params["expected_version"] == "1"


def test_lost_draft_save_retries_exactly_the_same_payload() -> None:
    bodies: list[bytes] = []

    def respond(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        if len(bodies) == 1:
            raise httpx.ReadError("response lost after save", request=request)
        return httpx.Response(200, json=DRAFT)

    with AgentGitClient(
        "http://draft.test",
        "reviewer",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
        sleep=lambda _: None,
    ) as client:
        result = client.proposals.save_draft(
            repo="team/app",
            id="proposal",
            expected_version=0,
            head="reviewed",
            decision="comment",
            body="Private notes",
        )
    assert result.body == "Private notes"
    assert len(bodies) == 2
    assert bodies[0] == bodies[1]


def test_clear_retries_the_same_query_version_even_when_zero() -> None:
    urls: list[str] = []

    def respond(request: httpx.Request) -> httpx.Response:
        urls.append(str(request.url))
        assert request.content == b""
        if len(urls) == 1:
            raise httpx.ReadError("response lost after clear", request=request)
        return httpx.Response(
            200, json={**DRAFT, "present": False, "body": "", "head": "", "decision": "", "version": 2}
        )

    with AgentGitClient(
        "http://draft.test",
        "reviewer",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
        sleep=lambda _: None,
    ) as client:
        result = client.proposals.clear_draft(repo="team/app", id="proposal", expected_version=0)
    assert not result.present
    assert result.version == 2
    assert urls[0] == urls[1]
    assert len(urls) == 2
    assert httpx.URL(urls[0]).params["expected_version"] == "0"


def test_async_draft_save_uses_the_same_guard_and_retry_contract() -> None:
    async def scenario() -> None:
        bodies: list[bytes] = []

        async def respond(request: httpx.Request) -> httpx.Response:
            bodies.append(request.content)
            if len(bodies) == 1:
                raise httpx.ReadError("response lost after save", request=request)
            return httpx.Response(200, json=DRAFT)

        async def no_sleep(_delay: float) -> None:
            pass

        async with AsyncAgentGitClient(
            "http://draft.test",
            "reviewer",
            http_client=httpx.AsyncClient(transport=httpx.MockTransport(respond)),
            sleep=no_sleep,
        ) as client:
            result = await client.proposals.save_draft(
                repo="team/app",
                id="proposal",
                expected_version=0,
                head="reviewed",
                decision="comment",
                body="Private notes",
            )
        assert result.body == "Private notes"
        assert len(bodies) == 2
        assert bodies[0] == bodies[1]

    asyncio.run(scenario())
