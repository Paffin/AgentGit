import json

import httpx
import pytest

from agentgit import AgentGitClient, CallOptions, RetryPolicy, UnavailableError


def test_sync_sends_inspected_head_and_preserves_blob_resolution() -> None:
    requests: list[httpx.Request] = []

    def respond(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        return httpx.Response(
            200, json={"status": "merged", "head": "merged", "stream_head": "inspected", "conflicts": []}
        )

    with AgentGitClient(
        "http://sync.test", "test", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        result = client.workspaces.sync(
            repo="team/app",
            name="work",
            expected_stream_head="inspected",
            resolutions=[
                {
                    "path": "image.bin",
                    "op": "put",
                    "blob_id": "original-blob",
                    "file_type": "regular",
                    "executable": True,
                }
            ],
        )
    assert result.stream_head == "inspected"
    assert json.loads(requests[0].content)["expected_stream_head"] == "inspected"
    assert json.loads(requests[0].content)["resolutions"][0]["blob_id"] == "original-blob"


def test_older_sync_response_defaults_missing_stream_head() -> None:
    def respond(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"status": "up_to_date", "head": "old", "conflicts": []})

    with AgentGitClient(
        "http://sync.test", "test", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        result = client.workspaces.sync(repo="team/app", name="work")
    assert result.stream_head == ""


@pytest.mark.parametrize("pin", [None, "", "inspected"])
def test_only_nonempty_pins_retry_lost_sync_responses(pin: str | None) -> None:
    bodies: list[bytes] = []

    def respond(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        if len(bodies) == 1:
            raise httpx.ReadError("response lost after applying the merge", request=request)
        return httpx.Response(
            200, json={"status": "up_to_date", "head": "merged", "stream_head": "inspected", "conflicts": []}
        )

    with AgentGitClient(
        "http://sync.test", "test", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        if pin:
            assert (
                client.workspaces.sync(repo="team/app", name="work", expected_stream_head=pin).head
                == "merged"
            )
            assert len(bodies) == 2
            assert bodies[0] == bodies[1]
        else:
            with pytest.raises(UnavailableError) as error:
                client.workspaces.sync(repo="team/app", name="work", expected_stream_head=pin)
            assert error.value.attempts == 1
            assert error.value.outcome_unknown
            assert len(bodies) == 1


def test_unpinned_sync_retains_explicit_unsafe_retry_opt_in() -> None:
    calls = 0

    def respond(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise httpx.ReadError("response lost", request=request)
        return httpx.Response(200, json={"status": "up_to_date", "head": "done", "conflicts": []})

    with AgentGitClient(
        "http://sync.test", "test", http_client=httpx.Client(transport=httpx.MockTransport(respond))
    ) as client:
        result = client.workspaces.sync(
            repo="team/app", name="work", options=CallOptions(retry=RetryPolicy(retry_unsafe=True))
        )
    assert result.head == "done"
    assert calls == 2
