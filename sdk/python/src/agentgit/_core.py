"""Transport-independent logic shared by the synchronous and the asynchronous client."""

from __future__ import annotations

import json
import secrets
import string
import time
from collections.abc import Mapping
from dataclasses import dataclass
from email.utils import parsedate_to_datetime
from typing import Any
from urllib.parse import quote, urlencode

from agentgit._codec import encode
from agentgit._operation import OperationInfo, RetryClass
from agentgit._options import CallOptions, RetryPolicy
from agentgit.errors import (
    AgentGitError,
    DeadlineExceededError,
    UnavailableError,
    error_from_info,
    error_from_status,
)

DEFAULT_TIMEOUT_MS = 30_000
_ALPHABET = string.ascii_letters + string.digits
_QUERY_CSV = frozenset({"topics", "repos"})

# Errors the server marks retryable but where sending the identical request again cannot succeed:
# ``stream_moved`` means a compare-and-swap lost against a newer version; the caller must re-read.
NEEDS_NEW_REQUEST = frozenset({"stream_moved"})


def random_token(length: int) -> str:
    """A random token made of ``[A-Za-z0-9]``, valid as request id and idempotency key."""
    return "".join(secrets.choice(_ALPHABET) for _ in range(length))


@dataclass
class BuiltRequest:
    url: str
    method: str
    json_body: bytes | None
    has_json_body: bool


def _scalar(value: Any, what: str) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (str, int)):
        return str(value)
    raise TypeError(f"{what} must be a string, number or boolean")


def build_request(op: OperationInfo, params: Mapping[str, Any], base_url: str) -> BuiltRequest:
    """Maps a flat parameter dict onto the route: path placeholders, query string or JSON body."""
    where = f"{op.namespace}.{op.name}"
    used: set[str] = set()
    repo_scoped = "owner" in op.path_params and "rname" in op.path_params
    repo_parts: tuple[str, str] | None = None
    if repo_scoped:
        repo = params.get("repo")
        parts = repo.split("/") if isinstance(repo, str) else []
        if len(parts) != 2 or not parts[0] or not parts[1]:
            raise ValueError(f'{where}: "repo" must look like "owner/name", got {repo!r}')
        repo_parts = (parts[0], parts[1])
        used.add("repo")

    path = op.route
    for name in op.path_params:
        if repo_parts and name == "owner":
            value = repo_parts[0]
        elif repo_parts and name == "rname":
            value = repo_parts[1]
        else:
            used.add(name)
            raw = params.get(name)
            if raw is None or raw == "":
                raise ValueError(f'{where}: missing required field "{name}"')
            value = _scalar(raw, f'{where}: field "{name}"')
        if "{*" + name + "}" in path:
            text = value.lstrip("/")
            if not text:
                raise ValueError(f'{where}: missing required field "{name}"')
            path = path.replace("{*" + name + "}", "/".join(quote(s, safe="") for s in text.split("/")))
        else:
            path = path.replace("{" + name + "}", quote(value, safe=""))

    rest = {k: v for k, v in params.items() if k not in used and v is not None}
    url = base_url + path
    body: bytes | None = None
    has_body = op.body == "json"
    if has_body:
        body = json.dumps(encode(rest), separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    elif op.body in ("none", "bytes") and rest:
        pairs: list[tuple[str, str]] = []
        for key, value in rest.items():
            if isinstance(value, (list, tuple)):
                if key in _QUERY_CSV:
                    pairs.append((key, ",".join(_scalar(v, f'field "{key}"') for v in value)))
                else:
                    pairs.extend((key, _scalar(v, f'field "{key}"')) for v in value)
            elif isinstance(value, (Mapping, set)) or hasattr(value, "__dataclass_fields__"):
                raise TypeError(f'{where}: field "{key}" cannot be sent in a query string')
            else:
                pairs.append((key, _scalar(value, f'field "{key}"')))
        if pairs:
            url += "?" + urlencode(pairs)
    return BuiltRequest(url=url, method=op.http, json_body=body, has_json_body=has_body)


def effective_retry_class(op: OperationInfo, params: Mapping[str, Any]) -> RetryClass:
    if op.retry == "unsafe" and op.retry_when_field is not None:
        # For example workspace.modify: with ``expected_head`` a second application is refused, never doubled.
        guard = params.get(op.retry_when_field)
        if guard is not None and guard != "":
            return "natural"
    return op.retry


def resolve_retry(default: RetryPolicy, override: RetryPolicy | bool | None) -> RetryPolicy:
    if override is False:
        return RetryPolicy(
            max_attempts=1,
            base_delay_ms=default.base_delay_ms,
            max_delay_ms=default.max_delay_ms,
            max_retry_after_ms=default.max_retry_after_ms,
            retry_unsafe=default.retry_unsafe,
        )
    if isinstance(override, RetryPolicy):
        return override
    return default


def should_retry(err: AgentGitError, retry_class: RetryClass, policy: RetryPolicy, attempt: int) -> bool:
    """A request the server rejected before running it (rate limit) can always be repeated; anything
    else only if repeating it cannot apply it twice."""
    safe = (
        retry_class != "unsafe" or policy.retry_unsafe or (err.code == "rate_limited" and err.status == 429)
    )
    return err.retryable and safe and err.code not in NEEDS_NEW_REQUEST and attempt < policy.max_attempts


def retry_delay_seconds(err: AgentGitError, attempt: int, policy: RetryPolicy, rand: float) -> float | None:
    """Full jitter over an exponentially growing window. A server-supplied delay is a floor, never
    shortened: many callers told the same delay would otherwise all return at once."""
    ceiling = min(policy.max_delay_ms, policy.base_delay_ms * (1 << min(attempt - 1, 30)))
    jitter = rand * ceiling
    if err.retry_after_ms > 0:
        if err.retry_after_ms > policy.max_retry_after_ms:
            return None
        return round(err.retry_after_ms + jitter) / 1000
    return round(jitter) / 1000


def parse_retry_after(value: str | None) -> int:
    if not value:
        return 0
    try:
        return max(0, round(float(value) * 1000))
    except ValueError:
        pass
    try:
        when = parsedate_to_datetime(value)
    except (TypeError, ValueError):
        return 0
    return max(0, round(when.timestamp() * 1000 - time.time() * 1000))


def timeout_ms_for(params: Mapping[str, Any], options: CallOptions | None, default_ms: int) -> int:
    if options is not None and options.timeout_ms is not None:
        return options.timeout_ms
    wait = params.get("wait_ms")
    if isinstance(wait, int) and wait > 0:
        return max(default_ms, wait + 10_000)
    return default_ms


def grace_ms(timeout_ms: int) -> int:
    return min(1000, max(100, round(timeout_ms * 0.1))) if timeout_ms > 0 else 0


def request_headers(
    op: OperationInfo,
    *,
    request_id: str,
    token: str | None,
    run_id: str | None,
    timeout_ms: int | None,
    idempotency_key: str | None,
    base: Mapping[str, str],
    extra: Mapping[str, str] | None,
) -> dict[str, str]:
    headers: dict[str, str] = {"x-request-id": request_id}
    if not op.public and token:
        headers["authorization"] = f"Bearer {token}"
    if run_id:
        headers["ag-run"] = run_id
    if timeout_ms is not None and timeout_ms > 0:
        headers["request-timeout"] = str(timeout_ms)
    if idempotency_key:
        headers["idempotency-key"] = idempotency_key
    for source in (base, extra or {}):
        for k, v in source.items():
            headers[k.lower()] = v
    return headers


def error_from_response(
    status: int, text: str, headers: Mapping[str, str], op: OperationInfo, request_id: str
) -> AgentGitError:
    retry_after = parse_retry_after(headers.get("retry-after"))
    echoed = headers.get("x-request-id") or request_id
    try:
        parsed = json.loads(text)
    except ValueError:
        parsed = None
    info = parsed.get("error") if isinstance(parsed, dict) else None
    if isinstance(info, dict):
        return error_from_info(
            info, status=status, operation=op.id, request_id=echoed, retry_after_ms=retry_after
        )
    return error_from_status(status, text, operation=op.id, request_id=echoed, retry_after_ms=retry_after)


def network_error(exc: BaseException, op: OperationInfo) -> AgentGitError:
    detail = str(exc) or type(exc).__name__
    return UnavailableError(
        f"{op.id}: network error ({detail})",
        status=0,
        retryable=True,
        operation=op.id,
        outcome_unknown=True,
    )


def timeout_error(op: OperationInfo, timeout_ms: int, request_id: str) -> AgentGitError:
    return DeadlineExceededError(
        f"{op.id}: no answer within {timeout_ms} ms; the operation may still complete",
        status=0,
        retryable=True,
        request_id=request_id,
        operation=op.id,
        outcome_unknown=True,
    )


def decode_json_body(text: str, op: OperationInfo, status: int, request_id: str) -> Any:
    if text == "":
        return {}
    try:
        return json.loads(text)
    except ValueError as e:
        raise AgentGitError(
            f"{op.id}: the server answered {status} with a body that is not JSON",
            code="internal",
            status=status,
            request_id=request_id,
            operation=op.id,
        ) from e
