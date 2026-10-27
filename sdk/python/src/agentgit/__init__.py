"""Python client for the AgentGit Wire Protocol (AGWP v1).

Two clients with the same surface: :class:`AgentGitClient` (synchronous) and
:class:`AsyncAgentGitClient` (asyncio). Every operation of ``docs/protocol/openapi.json`` is a
method of a namespace named after the operation id (``proposal.create`` is
``client.proposals.create``); every message is a dataclass in :mod:`agentgit.models`.
"""

from __future__ import annotations

from agentgit import models
from agentgit._generated.operations import NAMESPACE_METHODS, OPERATIONS, PAGINATED
from agentgit._operation import OperationInfo, RetryClass
from agentgit._options import CallOptions, ReconnectPolicy, RetryPolicy
from agentgit.changeset import (
    BuiltChangeset,
    DeleteOp,
    PutOp,
    assert_repo_path,
    build_changeset,
)
from agentgit.client import (
    PROTOCOL_VERSION,
    AgentGitClient,
    AsyncAgentGitClient,
    PutBlobResult,
    UploadResult,
)
from agentgit.errors import (
    AbortedError,
    AgentGitError,
    AlreadyExistsError,
    DeadlineExceededError,
    FailedPreconditionError,
    IdempotencyKeyReusedError,
    InternalError,
    InvalidArgumentError,
    LeaseConflictError,
    NotFoundError,
    ObjectCorruptError,
    ObjectMissingError,
    PermissionDeniedError,
    QuotaExceededError,
    RateLimitedError,
    StaleCandidateError,
    StreamMovedError,
    TooLargeError,
    UnauthenticatedError,
    UnavailableError,
    UnimplementedError,
)
from agentgit.files import delete_file, put_file
from agentgit.hashing import (
    Envelope,
    StoredObject,
    blob_id,
    blob_object,
    decode_base64,
    decode_envelope,
    encode_base64,
    encode_envelope,
    object_id,
)
from agentgit.signing import (
    AgentKey,
    agent_key_from_seed,
    attestation_signing_bytes,
    canonical_json,
    canonical_provenance_bytes,
    generate_agent_key,
    sign_attestation,
    sign_provenance,
    sign_with_domain,
    signing_message,
    verify_provenance,
    verify_with_domain,
)
from agentgit.sse import SseFrame, SseParser

__version__ = "0.3.0-beta.1"

__all__ = [
    "NAMESPACE_METHODS",
    "OPERATIONS",
    "PAGINATED",
    "PROTOCOL_VERSION",
    "AbortedError",
    "AgentGitClient",
    "AgentGitError",
    "AgentKey",
    "AlreadyExistsError",
    "AsyncAgentGitClient",
    "BuiltChangeset",
    "CallOptions",
    "DeadlineExceededError",
    "DeleteOp",
    "Envelope",
    "FailedPreconditionError",
    "IdempotencyKeyReusedError",
    "InternalError",
    "InvalidArgumentError",
    "LeaseConflictError",
    "NotFoundError",
    "ObjectCorruptError",
    "ObjectMissingError",
    "OperationInfo",
    "PermissionDeniedError",
    "PutBlobResult",
    "PutOp",
    "QuotaExceededError",
    "RateLimitedError",
    "ReconnectPolicy",
    "RetryClass",
    "RetryPolicy",
    "SseFrame",
    "SseParser",
    "StaleCandidateError",
    "StoredObject",
    "StreamMovedError",
    "TooLargeError",
    "UnauthenticatedError",
    "UnavailableError",
    "UnimplementedError",
    "UploadResult",
    "__version__",
    "agent_key_from_seed",
    "assert_repo_path",
    "attestation_signing_bytes",
    "blob_id",
    "blob_object",
    "build_changeset",
    "canonical_json",
    "canonical_provenance_bytes",
    "decode_base64",
    "decode_envelope",
    "delete_file",
    "encode_base64",
    "encode_envelope",
    "generate_agent_key",
    "models",
    "object_id",
    "put_file",
    "sign_attestation",
    "sign_provenance",
    "sign_with_domain",
    "signing_message",
    "verify_provenance",
    "verify_with_domain",
]
