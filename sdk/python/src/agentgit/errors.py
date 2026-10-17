"""Errors: one hierarchy, one class per stable AGWP error code."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any


class AgentGitError(Exception):
    """An error answered by the server (``{"error": ErrorInfo}``) or raised by the client for a
    transport failure.

    Attributes:
        code: the stable machine-readable code (``not_found``, ``stream_moved``, ...). Unknown codes
            are passed through.
        status: HTTP status; ``0`` when no response was received (network failure, local deadline).
        retryable: whether the server (or the client, for transport failures) considers a repeat
            worthwhile.
        retry_after_ms: server-suggested wait in milliseconds (``0`` = none).
        request_id: correlation id (``X-Request-Id``), also echoed by the server in every error.
        details: structured details.
        operation: the operation id, for example ``proposal.create``.
        attempts: how many attempts were made before this error was raised.
        outcome_unknown: the request may have been applied although no usable answer arrived
            (deadline, dropped connection). Ask for the outcome (``operations.get``) before repeating
            a call that is not idempotent.
    """

    code: str = "internal"

    def __init__(
        self,
        message: str,
        *,
        code: str | None = None,
        status: int = 0,
        retryable: bool = False,
        retry_after_ms: int = 0,
        request_id: str = "",
        details: Mapping[str, str] | None = None,
        operation: str | None = None,
        attempts: int = 1,
        outcome_unknown: bool = False,
    ) -> None:
        super().__init__(message)
        self.message = message
        if code is not None:
            self.code = code
        self.status = status
        self.retryable = retryable
        self.retry_after_ms = retry_after_ms
        self.request_id = request_id
        self.details: dict[str, str] = dict(details or {})
        self.operation = operation
        self.attempts = attempts
        self.outcome_unknown = outcome_unknown

    def is_code(self, *codes: str) -> bool:
        """``error.is_code("stream_moved", "aborted")`` - true when the code is one of the given ones."""
        return self.code in codes

    def __str__(self) -> str:
        where = f" [{self.operation}]" if self.operation else ""
        rid = f" (request {self.request_id})" if self.request_id else ""
        return f"{self.code}: {self.message}{where}{rid}"


class UnauthenticatedError(AgentGitError):
    code = "unauthenticated"


class PermissionDeniedError(AgentGitError):
    code = "permission_denied"


class NotFoundError(AgentGitError):
    code = "not_found"


class AlreadyExistsError(AgentGitError):
    code = "already_exists"


class InvalidArgumentError(AgentGitError):
    code = "invalid_argument"


class FailedPreconditionError(AgentGitError):
    code = "failed_precondition"


class StreamMovedError(AgentGitError):
    code = "stream_moved"


class IdempotencyKeyReusedError(AgentGitError):
    code = "idempotency_key_reused"


class ObjectMissingError(AgentGitError):
    code = "object_missing"


class ObjectCorruptError(AgentGitError):
    code = "object_corrupt"


class TooLargeError(AgentGitError):
    code = "too_large"


class RateLimitedError(AgentGitError):
    code = "rate_limited"


class QuotaExceededError(AgentGitError):
    code = "quota_exceeded"


class UnavailableError(AgentGitError):
    code = "unavailable"


class DeadlineExceededError(AgentGitError):
    code = "deadline_exceeded"


class AbortedError(AgentGitError):
    code = "aborted"


class LeaseConflictError(AgentGitError):
    code = "lease_conflict"


class StaleCandidateError(AgentGitError):
    code = "stale_candidate"


class UnimplementedError(AgentGitError):
    code = "unimplemented"


class InternalError(AgentGitError):
    code = "internal"


ERROR_CLASSES: dict[str, type[AgentGitError]] = {
    cls.code: cls
    for cls in (
        UnauthenticatedError,
        PermissionDeniedError,
        NotFoundError,
        AlreadyExistsError,
        InvalidArgumentError,
        FailedPreconditionError,
        StreamMovedError,
        IdempotencyKeyReusedError,
        ObjectMissingError,
        ObjectCorruptError,
        TooLargeError,
        RateLimitedError,
        QuotaExceededError,
        UnavailableError,
        DeadlineExceededError,
        AbortedError,
        LeaseConflictError,
        StaleCandidateError,
        UnimplementedError,
        InternalError,
    )
}

_STATUS_CODES = {
    400: "invalid_argument",
    401: "unauthenticated",
    403: "permission_denied",
    404: "not_found",
    412: "failed_precondition",
    413: "too_large",
    429: "rate_limited",
    501: "unimplemented",
    502: "unavailable",
    503: "unavailable",
    504: "deadline_exceeded",
}


def code_for_status(status: int) -> str:
    return _STATUS_CODES.get(status, "internal" if status >= 500 else "http_error")


def make_error(
    code: str,
    message: str,
    **kwargs: Any,
) -> AgentGitError:
    """The most specific subclass for ``code`` (plain :class:`AgentGitError` for unknown codes)."""
    cls = ERROR_CLASSES.get(code, AgentGitError)
    return cls(message, code=code, **kwargs)


def error_from_info(
    info: Mapping[str, Any],
    *,
    status: int,
    operation: str | None,
    request_id: str = "",
    retry_after_ms: int = 0,
) -> AgentGitError:
    """Builds an error from the ``ErrorInfo`` of a response body."""
    code = str(info.get("code") or code_for_status(status))
    wait = info.get("retry_after_ms")
    return make_error(
        code,
        str(info.get("message") or f"request failed with HTTP {status}"),
        status=status,
        retryable=bool(info.get("retryable", False)),
        retry_after_ms=int(wait) if isinstance(wait, int) and wait > 0 else retry_after_ms,
        request_id=str(info.get("request_id") or request_id),
        details={str(k): str(v) for k, v in (info.get("details") or {}).items()},
        operation=operation,
        outcome_unknown=code == "deadline_exceeded",
    )


def error_from_status(
    status: int, body: str, *, operation: str | None, request_id: str = "", retry_after_ms: int = 0
) -> AgentGitError:
    """The error for a response that is not the documented JSON envelope (proxies, 405, ...)."""
    code = code_for_status(status)
    text = body.strip()[:200]
    return make_error(
        code,
        f"HTTP {status}: {text}" if text else f"HTTP {status}",
        status=status,
        retryable=code in ("unavailable", "rate_limited", "deadline_exceeded"),
        retry_after_ms=retry_after_ms,
        request_id=request_id,
        operation=operation,
        outcome_unknown=status >= 500,
    )
