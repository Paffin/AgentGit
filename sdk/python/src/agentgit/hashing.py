"""Content ids and the object envelope (docs/architecture/object-format.md)."""

from __future__ import annotations

import base64
import hashlib
import struct
from typing import NamedTuple

OBJECT_KIND_BLOB = 1
OBJECT_KIND_CHANGESET = 2
OBJECT_KIND_TREE = 3
OBJECT_VERSION = 1

_ID_DOMAIN = b"agentgit.object\x00"
_ENVELOPE_MAGIC = b"AGOB"


def object_id(kind: int, version: int, payload: bytes) -> str:
    """``SHA-256("agentgit.object\\0" || kind:u8 || version:u16be || length:u64be || payload)``, lowercase hex.

    The kind and version are part of the preimage, so a blob never collides with a changeset.
    """
    h = hashlib.sha256()
    h.update(_ID_DOMAIN)
    h.update(struct.pack(">BHQ", kind, version, len(payload)))
    h.update(payload)
    return h.hexdigest()


def blob_id(content: bytes | str) -> str:
    """The content id of a blob (what ``FileChange.blob_id`` and changeset puts refer to). Text is UTF-8 encoded."""
    data = content.encode("utf-8") if isinstance(content, str) else bytes(content)
    return object_id(OBJECT_KIND_BLOB, OBJECT_VERSION, data)


def encode_envelope(kind: int, payload: bytes, version: int = OBJECT_VERSION) -> bytes:
    """The stored form of an object: ``"AGOB" || kind:u8 || version:u16be || payload``."""
    return _ENVELOPE_MAGIC + struct.pack(">BH", kind, version) + payload


class Envelope(NamedTuple):
    kind: int
    version: int
    payload: bytes


def decode_envelope(envelope: bytes) -> Envelope:
    """Splits an envelope; raises ``ValueError`` when it is not one."""
    if len(envelope) < 7 or envelope[:4] != _ENVELOPE_MAGIC:
        raise ValueError("not an AgentGit object envelope")
    kind, version = struct.unpack(">BH", envelope[4:7])
    return Envelope(kind, version, envelope[7:])


class StoredObject(NamedTuple):
    id: str
    envelope: bytes
    """The bytes for ``objects.put``."""


def blob_object(content: bytes | str) -> StoredObject:
    """The envelope of a blob, ready for ``objects.put``, together with its id."""
    data = content.encode("utf-8") if isinstance(content, str) else bytes(content)
    return StoredObject(
        object_id(OBJECT_KIND_BLOB, OBJECT_VERSION, data), encode_envelope(OBJECT_KIND_BLOB, data)
    )


def encode_base64(data: bytes) -> str:
    """Standard base64 with padding, as used by ``bytes`` fields in the JSON mapping."""
    return base64.b64encode(data).decode("ascii")


def decode_base64(text: str) -> bytes:
    """Decodes standard base64 (padding optional, ASCII whitespace ignored). Raises ``ValueError`` on bad input."""
    clean = "".join(text.split()).rstrip("=")
    if len(clean) % 4 == 1:
        raise ValueError("invalid base64 length")
    try:
        return base64.b64decode(clean + "=" * (-len(clean) % 4), validate=True)
    except ValueError as e:  # binascii.Error is a ValueError
        raise ValueError(f"invalid base64: {e}") from e
