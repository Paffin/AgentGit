"""Static description of one HTTP operation (the table is generated from the OpenAPI document)."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

RetryClass = Literal["read", "natural", "keyed", "unsafe"]
"""How a request is safe to repeat.

``read``: it only reads. ``natural``: repeating it has the same effect as making it once.
``keyed``: the server records the outcome under the ``Idempotency-Key`` header. ``unsafe``: a repeat
may be applied twice, so it is never retried automatically.
"""


@dataclass(frozen=True)
class OperationInfo:
    id: str
    http: str
    """``GET``, ``POST``, ``PUT``, ``PATCH`` or ``DELETE``."""
    route: str
    """Route template, e.g. ``/v1/repos/{owner}/{rname}/proposals/{id}``."""
    namespace: str
    name: str
    summary: str
    path_params: tuple[str, ...]
    query_params: tuple[str, ...]
    body: Literal["json", "bytes", "none"]
    response: Literal["json", "bytes", "sse"]
    retry: RetryClass
    idempotency_key: bool
    """Whether the operation accepts an idempotency key (message field or HTTP header)."""
    public: bool
    """Needs no credentials."""
    retry_when_field: str | None = None
    """``natural`` only when the request carries this field (an optimistic-concurrency guard)."""
