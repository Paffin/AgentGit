"""Pure client-side behaviour that needs no server: request building, options, error mapping."""

from __future__ import annotations

import json
from typing import Any

import httpx
import pytest

from agentgit import OPERATIONS, AgentGitClient, AgentGitError, AsyncAgentGitClient, models
from agentgit._core import build_request
from agentgit.errors import ERROR_CLASSES, error_from_info, error_from_status

BASE = "http://srv.test"


def build(op_id: str, **params: Any) -> Any:
    return build_request(OPERATIONS[op_id], params, BASE)


def query(url: str) -> dict[str, str]:
    return dict(httpx.URL(url).params)


class TestBuildingRequests:
    def test_splits_repo_into_path_segments_and_sends_the_rest_as_a_query(self) -> None:
        b = build("history.log", repo="acme/web", rev="main~2", path="src/a b.rs", page_size=5)
        url = httpx.URL(b.url)
        assert b.method == "GET"
        assert url.path == "/v1/repos/acme/web/log"
        assert query(b.url) == {"rev": "main~2", "path": "src/a b.rs", "page_size": "5"}
        assert b.json_body is None

    def test_keeps_free_form_names_in_the_query_string_never_in_the_path(self) -> None:
        b = build("stream.head", repo="acme/web", stream="feature/x y#1?", wait_ms=5)
        assert httpx.URL(b.url).path == "/v1/repos/acme/web/streams/head"
        assert query(b.url)["stream"] == "feature/x y#1?"

    def test_encodes_each_segment_of_a_file_path_and_drops_a_leading_slash(self) -> None:
        b = build("tree.file", repo="acme/web", path="/dir one/файл #1?.txt", rev="main")
        assert (
            b.url
            == f"{BASE}/v1/repos/acme/web/files/dir%20one/%D1%84%D0%B0%D0%B9%D0%BB%20%231%3F.txt?rev=main"
        )

    def test_topics_are_comma_joined_other_arrays_repeat_booleans_are_lowercase_none_is_skipped(self) -> None:
        r = build(
            "events.replay", repo="acme/web", topics=["stream", "proposal.create"], after_seq=0, limit=None
        )
        assert query(r.url) == {"topics": "stream,proposal.create", "after_seq": "0"}
        n = build("session.negotiate", supported_versions=["a", "b"], client_name="x")
        assert httpx.URL(n.url).params.get_list("supported_versions") == ["a", "b"]
        assert query(build("task.list", repo="acme/web", ready_only=True).url)["ready_only"] == "true"

    def test_body_fields_are_json_without_the_path_parameters(self) -> None:
        b = build("proposal.review", repo="acme/web", id="prop_1", decision="approve", body="ok")
        assert b.url == f"{BASE}/v1/repos/acme/web/proposals/prop_1/reviews"
        assert json.loads(b.json_body) == {"decision": "approve", "body": "ok"}
        g = build("admin.roles.grant", subject="user:a", repo="acme/*", role="reader")
        assert g.url == f"{BASE}/v1/roles"
        assert json.loads(g.json_body) == {
            "subject": "user:a",
            "repo": "acme/*",
            "role": "reader",
        }  # repo is a plain field

    def test_nested_messages_are_encoded_with_wire_names_and_bytes_become_base64(self) -> None:
        change = models.FileChange(path="p", op="put", content=b"x")  # type: ignore[arg-type]
        b = build("workspace.modify", repo="a/b", name="w", changes=[change, {"path": "q", "op": "delete"}])
        body = json.loads(b.json_body)
        assert body["changes"][0]["content"] == "eA=="
        assert body["changes"][1] == {"path": "q", "op": "delete"}
        d = build("tree.diff", repo="a/b", **{"from": "x"}, to="y")
        assert json.loads(d.json_body) == {"from": "x", "to": "y"}

    def test_delete_routes_take_path_and_query_only(self) -> None:
        b = build("admin.roles.revoke", subject="user:a", repo="acme/web", role="reader")
        assert b.method == "DELETE"
        assert b.json_body is None
        assert query(b.url)["repo"] == "acme/web"
        assert (
            build("lease.release", repo="acme/web", lease_id="lease_1").url
            == f"{BASE}/v1/repos/acme/web/leases/lease_1"
        )

    def test_rejects_nonsense_early(self) -> None:
        with pytest.raises(ValueError, match="owner/name"):
            build("repo.describe", repo="a/b/c")
        with pytest.raises(ValueError, match="owner/name"):
            build("repo.describe")
        with pytest.raises(ValueError, match='"id"'):
            build("proposal.get", repo="a/b")
        with pytest.raises(TypeError, match="query string"):
            build("repo.list", prefix={"nested": 1})


class TestClientOptions:
    @staticmethod
    def echo(calls: list[httpx.Request]) -> httpx.Client:
        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            return httpx.Response(200, json={})

        return httpx.Client(transport=httpx.MockTransport(handler))

    def test_a_token_function_is_asked_for_a_fresh_credential_on_every_request(self) -> None:
        calls: list[httpx.Request] = []
        n = 0

        def token() -> str:
            nonlocal n
            n += 1
            return f"tok-{n}"

        with AgentGitClient(BASE, token, http_client=self.echo(calls)) as c:
            c.session.whoami()
            c.session.whoami()
        assert [r.headers["authorization"] for r in calls] == ["Bearer tok-1", "Bearer tok-2"]

    def test_keeps_a_path_prefix_of_the_base_url_and_trims_trailing_slashes(self) -> None:
        calls: list[httpx.Request] = []
        with AgentGitClient(f"{BASE}/agentgit//", "t", http_client=self.echo(calls)) as c:
            c.repos.describe(repo="a/b")
            assert c.base_url == f"{BASE}/agentgit"
        assert str(calls[0].url) == f"{BASE}/agentgit/v1/repos/a/b"

    def test_with_run_and_with_token_return_new_clients_sharing_the_pool(self) -> None:
        from agentgit import CallOptions

        calls: list[httpx.Request] = []
        http = self.echo(calls)
        a = AgentGitClient(BASE, "a", http_client=http)
        b = a.with_run("run_1").with_token("b")
        a.session.whoami()
        b.session.whoami()
        assert "ag-run" not in calls[0].headers
        assert calls[1].headers["ag-run"] == "run_1"
        assert calls[1].headers["authorization"] == "Bearer b"
        a.session.whoami(options=CallOptions(run_id="run_2", headers={"X-Extra": "1"}))
        assert calls[2].headers["ag-run"] == "run_2"
        assert calls[2].headers["x-extra"] == "1"
        b.close()  # a derived client does not own the pool
        a.session.whoami()
        assert len(calls) == 4
        http.close()

    def test_objects_put_sends_bytes_as_an_octet_stream(self) -> None:
        seen: list[tuple[str, int]] = []

        def handler(request: httpx.Request) -> httpx.Response:
            seen.append((request.headers["content-type"], len(request.content)))
            return httpx.Response(200, json={"object_id": "x", "stored": True})

        with AgentGitClient(BASE, "t", http_client=httpx.Client(transport=httpx.MockTransport(handler))) as c:
            assert c.objects.put(repo="a/b", object_id="x", data=b"\0" * 5).stored
        assert seen == [("application/octet-stream", 5)]

    def test_refuses_to_be_built_without_a_base_url(self) -> None:
        with pytest.raises(ValueError, match="base_url"):
            AgentGitClient("")
        with pytest.raises(ValueError, match="base_url"):
            AsyncAgentGitClient("")

    def test_negotiate_sends_the_versions_it_speaks_as_a_repeated_parameter(self) -> None:
        calls: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            return httpx.Response(
                200, json={"protocol_version": "agwp.v1", "features": [], "auth_methods": []}
            )

        with AgentGitClient(BASE, http_client=httpx.Client(transport=httpx.MockTransport(handler))) as c:
            c.negotiate(client_name="x", client_version="1")
        assert calls[0].url.query == b"supported_versions=agwp.v1&client_name=x&client_version=1"
        assert "authorization" not in calls[0].headers

    def test_call_dispatches_any_operation_by_id(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, json={"name": "a/b"})

        with AgentGitClient(BASE, "t", http_client=httpx.Client(transport=httpx.MockTransport(handler))) as c:
            assert c.call("repo.describe", repo="a/b").name == "a/b"

    def test_the_http_client_it_creates_is_closed_but_one_passed_in_is_not(self) -> None:
        mine = httpx.Client()
        AgentGitClient(BASE, http_client=mine).close()
        assert not mine.is_closed
        mine.close()
        c = AgentGitClient(BASE)
        c.close()
        assert c._http.is_closed


class TestErrors:
    def test_http_statuses_map_to_codes_and_transient_ones_are_retryable(self) -> None:
        def e(status: int) -> AgentGitError:
            return error_from_status(status, "", operation=None)

        assert (e(502).code, e(502).retryable, e(502).outcome_unknown) == ("unavailable", True, True)
        assert (e(504).code, e(504).retryable) == ("deadline_exceeded", True)
        assert (e(429).code, e(429).retryable) == ("rate_limited", True)
        assert (e(404).code, e(404).retryable) == ("not_found", False)
        assert (e(405).code, e(405).retryable) == ("http_error", False)
        assert (e(500).code, e(500).retryable) == ("internal", False)
        assert "Bad Gateway" in error_from_status(502, "<html>Bad Gateway</html>", operation=None).message

    def test_every_documented_code_has_a_subclass_and_unknown_codes_pass_through(self) -> None:
        documented = [
            "unauthenticated",
            "permission_denied",
            "not_found",
            "already_exists",
            "invalid_argument",
            "failed_precondition",
            "stream_moved",
            "idempotency_key_reused",
            "object_missing",
            "object_corrupt",
            "too_large",
            "rate_limited",
            "quota_exceeded",
            "unavailable",
            "deadline_exceeded",
            "aborted",
            "lease_conflict",
            "stale_candidate",
            "unimplemented",
            "internal",
        ]
        assert sorted(ERROR_CLASSES) == sorted(documented)
        for code in documented:
            err = error_from_info({"code": code, "message": "m"}, status=400, operation="x")
            assert type(err) is ERROR_CLASSES[code]
            assert isinstance(err, AgentGitError)
        future = error_from_info(
            {"code": "brand_new", "message": "m", "details": {"k": "v"}}, status=418, operation="x"
        )
        assert type(future) is AgentGitError
        assert (future.code, future.details) == ("brand_new", {"k": "v"})

    def test_an_error_is_a_real_exception_with_a_useful_message(self) -> None:
        err = error_from_info(
            {
                "code": "stream_moved",
                "message": "moved",
                "retryable": True,
                "retry_after_ms": 5,
                "request_id": "req_1",
            },
            status=409,
            operation="stream.cas",
        )
        assert isinstance(err, Exception)
        assert str(err) == "stream_moved: moved [stream.cas] (request req_1)"
        assert (err.status, err.retry_after_ms, err.attempts, err.outcome_unknown) == (409, 5, 1, False)
