"""Review-routing request and retry contracts, independent of server state."""

from __future__ import annotations

import asyncio
import json

import httpx
import pytest
from examples.review_requests import request_review, reviews_requested_from

from agentgit import AgentGitClient, AgentGitError, AsyncAgentGitClient


def test_sends_the_complete_inspected_set_and_retries_a_lost_response() -> None:
    requests: list[httpx.Request] = []

    def respond(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        if len(requests) == 1:
            raise httpx.ReadError("response lost after commit", request=request)
        return httpx.Response(200, json={"reviewers": ["user:carol"]})

    with AgentGitClient(
        "http://review.test",
        "test",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
        sleep=lambda _delay: None,
    ) as client:
        result = client.proposals.set_reviewers(
            repo="team/app",
            id="proposal #1",
            reviewers=["user:carol"],
            expected_reviewers=["user:bob"],
        )
    assert result.reviewers == ["user:carol"]
    assert len(requests) == 2
    assert requests[0].method == "PUT"
    assert requests[0].url.raw_path == b"/v1/repos/team/app/proposals/proposal%20%231/reviewers"
    assert requests[0].content == requests[1].content
    assert json.loads(requests[0].content) == {
        "reviewers": ["user:carol"],
        "expected_reviewers": ["user:bob"],
    }


def test_preserves_the_requested_reviewer_filter_while_paging() -> None:
    urls: list[httpx.URL] = []

    def respond(request: httpx.Request) -> httpx.Response:
        urls.append(request.url)
        return httpx.Response(
            200,
            json={
                "proposals": [{"id": f"p{len(urls)}"}],
                "next_page_token": "next" if len(urls) == 1 else "",
            },
        )

    with AgentGitClient(
        "http://review.test",
        "test",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
    ) as client:
        ids = [
            p.id
            for p in client.paginate(
                "proposal.list",
                repo="team/app",
                requested_reviewer="user:bob",
                page_size=1,
            )
        ]
    assert ids == ["p1", "p2"]
    assert [url.params["requested_reviewer"] for url in urls] == ["user:bob", "user:bob"]
    assert urls[1].params["page_token"] == "next"


def test_async_client_exposes_the_same_guarded_operation() -> None:
    async def scenario() -> None:
        def respond(request: httpx.Request) -> httpx.Response:
            assert json.loads(request.content) == {"reviewers": [], "expected_reviewers": ["user:bob"]}
            return httpx.Response(200, json={"reviewers": []})

        async with AsyncAgentGitClient(
            "http://review.test",
            "test",
            http_client=httpx.AsyncClient(transport=httpx.MockTransport(respond)),
        ) as client:
            result = await client.proposals.set_reviewers(
                repo="team/app",
                id="p1",
                reviewers=[],
                expected_reviewers=["user:bob"],
            )
            assert result.reviewers == []

    asyncio.run(scenario())


def test_old_servers_may_omit_requested_reviewers() -> None:
    def respond(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"proposal": {"id": "p1"}, "reviews": []})

    with AgentGitClient(
        "http://review.test",
        "test",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
    ) as client:
        detail = client.proposals.get(repo="team/app", id="p1")
    assert detail.proposal.id == "p1"
    assert detail.requested_reviewers == []


def test_helper_keeps_the_inspected_set_when_a_concurrent_edit_is_refused() -> None:
    requests: list[httpx.Request] = []

    def respond(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        if request.method == "GET":
            return httpx.Response(
                200, json={"proposal": {"id": "p1"}, "reviews": [], "requested_reviewers": ["user:bob"]}
            )
        return httpx.Response(
            412,
            json={
                "error": {"code": "failed_precondition", "message": "reviewers changed", "retryable": False}
            },
        )

    with AgentGitClient(
        "http://review.test",
        "test",
        http_client=httpx.Client(transport=httpx.MockTransport(respond)),
    ) as client:
        with pytest.raises(AgentGitError) as exc:
            request_review(client, "team/app", "p1", ["user:carol"])
    assert exc.value.status == 412
    assert [request.method for request in requests] == ["GET", "PUT"]
    assert json.loads(requests[1].content) == {
        "reviewers": ["user:carol"],
        "expected_reviewers": ["user:bob"],
    }


def test_filter_helper_refuses_unsupported_servers_before_listing() -> None:
    paths: list[str] = []

    def respond(request: httpx.Request) -> httpx.Response:
        paths.append(request.url.path)
        return httpx.Response(200, json={"protocol_version": "agwp.v1", "features": []})

    with AgentGitClient(
        "http://review.test", "test", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        with pytest.raises(ValueError, match="does not support requested-reviewer filtering"):
            list(reviews_requested_from(client, "team/app", "user:bob"))
    assert paths == ["/v1/capabilities"]
