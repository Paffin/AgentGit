"""Ed25519 keys and domain-separated signatures for provenance and attestations.

Everything signed is Ed25519 over ``"agentgit.sig.v1\\0" || len(domain):u8 || domain || payload``
(docs/protocol/agwp-v1.md, "Signing formats"). The provenance payload is the record as compact JSON
with keys sorted at every level and unset optionals as ``null``.
"""

from __future__ import annotations

import json
import os
import struct
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

PROVENANCE_SCHEMA = "agentgit.provenance.v1"
_SIG_PREFIX = b"agentgit.sig.v1\x00"


@dataclass(frozen=True)
class AgentKey:
    """An Ed25519 key pair for an agent. The private key never needs to leave the agent."""

    private_key: bytes
    """32-byte Ed25519 seed. Keep it secret."""
    public_key: bytes
    """32-byte public key."""

    @property
    def public_key_hex(self) -> str:
        """Lowercase hex of the public key: what ``agents.register`` expects as ``public_key``."""
        return self.public_key.hex()

    def __repr__(self) -> str:  # never print the seed
        return f"AgentKey(public_key={self.public_key_hex!r})"


def _seed(private_key: bytes | str) -> bytes:
    seed = bytes.fromhex(private_key) if isinstance(private_key, str) else bytes(private_key)
    if len(seed) != 32:
        raise ValueError("an Ed25519 private key is 32 bytes")
    return seed


def agent_key_from_seed(seed: bytes | str) -> AgentKey:
    """Rebuilds a key pair from a 32-byte seed (bytes or hex)."""
    raw = _seed(seed)
    public = (
        Ed25519PrivateKey.from_private_bytes(raw).public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    )
    return AgentKey(private_key=raw, public_key=public)


def generate_agent_key() -> AgentKey:
    """A fresh random key pair."""
    return agent_key_from_seed(os.urandom(32))


def signing_message(domain: str, message: bytes) -> bytes:
    """``"agentgit.sig.v1\\0" || len(domain):u8 || domain || message`` - the bytes that are actually signed."""
    d = domain.encode("utf-8")
    if not 1 <= len(d) <= 64:
        raise ValueError("a signature domain is 1..=64 bytes")
    return _SIG_PREFIX + bytes([len(d)]) + d + message


def sign_with_domain(domain: str, message: bytes, private_key: bytes | str) -> str:
    """Ed25519 signature (lowercase hex) of ``message`` under ``domain``."""
    key = Ed25519PrivateKey.from_private_bytes(_seed(private_key))
    return key.sign(signing_message(domain, message)).hex()


def verify_with_domain(domain: str, message: bytes, signature_hex: str, public_key: bytes | str) -> bool:
    """Verifies a hex signature over ``message`` under ``domain``."""
    try:
        pk = bytes.fromhex(public_key) if isinstance(public_key, str) else bytes(public_key)
        Ed25519PublicKey.from_public_bytes(pk).verify(
            bytes.fromhex(signature_hex), signing_message(domain, message)
        )
    except (InvalidSignature, ValueError):
        return False
    return True


def canonical_json(value: Any) -> str:
    """Compact JSON with keys sorted by code point (= UTF-8 bytes) at every level; integers only."""
    return json.dumps(_check_json(value), sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _check_json(value: Any) -> Any:
    if isinstance(value, float):
        raise TypeError("canonical JSON carries integers only")
    if isinstance(value, Mapping):
        return {str(k): _check_json(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_check_json(v) for v in value]
    return value


def _get(record: Any, key: str, default: Any = None) -> Any:
    if isinstance(record, Mapping):
        return record.get(key, default)
    return getattr(record, key, default)


def canonical_provenance_bytes(record: Any) -> bytes:
    """The bytes the server verifies for a provenance record.

    ``record`` is a ``ProvenanceRecord`` model or a mapping with the wire field names (``changeset``,
    ``agent``, ``run_id``, ``task_id``, ``model``, ``prompt_digest``, ``parameters``, ``checks_run``,
    ``inputs``, ``rationale``, ``created_at_ms``). Empty/absent optionals are ``null``;
    ``created_at_ms`` is signed as ``created_at``.
    """
    created = _get(record, "created_at_ms")
    if isinstance(created, bool) or not isinstance(created, int):
        raise TypeError("created_at_ms must be an integer")

    def opt(key: str) -> str | None:
        v = _get(record, key)
        return v if v else None

    body = {
        "schema": PROVENANCE_SCHEMA,
        "changeset": _get(record, "changeset"),
        "agent": _get(record, "agent"),
        "run_id": opt("run_id"),
        "task_id": opt("task_id"),
        "model": opt("model"),
        "prompt_digest": opt("prompt_digest"),
        "parameters": dict(_get(record, "parameters") or {}),
        "checks_run": list(_get(record, "checks_run") or []),
        "inputs": list(_get(record, "inputs") or []),
        "rationale": opt("rationale"),
        "created_at": created,
    }
    return canonical_json(body).encode("utf-8")


def sign_provenance(record: Any, private_key: bytes | str) -> str:
    """Signs a provenance record with the agent's own key; the result goes into ``provenance.submit(signature=...)``."""
    return sign_with_domain("provenance.v1", canonical_provenance_bytes(record), private_key)


def verify_provenance(record: Any, signature_hex: str, public_key: bytes | str) -> bool:
    """Checks a provenance signature the way the server does."""
    return verify_with_domain("provenance.v1", canonical_provenance_bytes(record), signature_hex, public_key)


def _varint(n: int) -> bytes:
    if n < 0:
        raise ValueError("negative varint")
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n == 0:
            out.append(b)
            return bytes(out)
        out.append(b | 0x80)


def _lp_string(text: str) -> bytes:
    raw = text.encode("utf-8")
    return _varint(len(raw)) + raw


def attestation_signing_bytes(attestation: Any) -> bytes:
    """Canonical signing bytes of an attestation (docs/architecture/object-format.md, "Signatures").

    ``attestation`` is an ``Attestation`` model or a mapping with ``id``, ``tree_id``, ``check``,
    ``result`` (``pass`` or ``fail``), ``policy_version``, ``environment_digest``, ``issuer`` and
    ``issued_at_ms``.
    """
    a = attestation
    return (
        _lp_string(_get(a, "id"))
        + bytes.fromhex(_get(a, "tree_id"))
        + _lp_string(_get(a, "check"))
        + bytes([1 if _get(a, "result") == "pass" else 2])
        + _varint(int(_get(a, "policy_version")))
        + _lp_string(_get(a, "environment_digest"))
        + _lp_string(_get(a, "issuer"))
        + struct.pack(">q", int(_get(a, "issued_at_ms")))
    )


def sign_attestation(attestation: Mapping[str, Any], key: AgentKey) -> dict[str, Any]:
    """Signs an attestation as a trusted worker; returns the full ``Attestation`` mapping for ``integration.attest``."""
    signature = sign_with_domain("attestation.v1", attestation_signing_bytes(attestation), key.private_key)
    return {**attestation, "public_key": key.public_key_hex, "signature": signature}
