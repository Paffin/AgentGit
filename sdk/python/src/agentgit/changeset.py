"""Client-side encoding of ``ChangesetV1`` (docs/architecture/object-format.md)."""

from __future__ import annotations

import re
import struct
from collections.abc import Sequence
from dataclasses import dataclass, field
from itertools import pairwise
from typing import Literal

from agentgit.hashing import (
    OBJECT_KIND_BLOB,
    OBJECT_KIND_CHANGESET,
    OBJECT_VERSION,
    StoredObject,
    blob_id,
    encode_envelope,
    object_id,
)

MAX_PARENTS = 16
MAX_OPS = 100_000
MAX_MESSAGE_BYTES = 64 * 1024

FileType = Literal["regular", "symlink"]

_IDENTITY = re.compile(r"^(user|agent|service|system):[A-Za-z0-9._@#+-]{1,128}$")
_NAMESPACE_SEGMENT = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
_HEX64 = re.compile(r"^[0-9a-f]{64}$")
_CONTROL = re.compile(r"[\x00-\x1f\x7f\\]")


@dataclass(frozen=True)
class PutOp:
    """Creates or replaces a file. Give the bytes in ``content`` or the id of a stored blob in ``blob``."""

    path: str
    content: bytes | str | None = None
    blob: str | None = None
    file_type: FileType = "regular"
    executable: bool = False


@dataclass(frozen=True)
class DeleteOp:
    path: str


@dataclass(frozen=True)
class BuiltChangeset:
    id: str
    """The changeset id: what you propose and what other changesets name as parent."""
    payload: bytes
    """The canonical ``ChangesetV1`` payload."""
    object: StoredObject
    """The changeset object itself, ready for ``objects.put``."""
    blobs: tuple[StoredObject, ...] = field(default_factory=tuple)
    """The blobs created from ``content``, de-duplicated; upload these before the changeset."""

    def objects(self) -> list[StoredObject]:
        """Everything to upload, blobs first."""
        return [*self.blobs, self.object]


def _varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n == 0:
            out.append(b)
            return bytes(out)
        out.append(b | 0x80)


def _string(text: str) -> bytes:
    raw = text.encode("utf-8")
    return _varint(len(raw)) + raw


def assert_repo_path(path: str) -> None:
    """Raises ``ValueError`` unless ``path`` is a valid repository path."""
    raw = path.encode("utf-8")
    if not 1 <= len(raw) <= 4096:
        raise ValueError(f"invalid path {path!r}: 1..=4096 bytes")
    if path.startswith("/"):
        raise ValueError(f"invalid path {path!r}: must be relative")
    for part in path.split("/"):
        if part in ("", ".", ".."):
            raise ValueError(f"invalid path {path!r}: empty, '.' or '..' component")
        if len(part.encode("utf-8")) > 255:
            raise ValueError(f"invalid path {path!r}: a component exceeds 255 bytes")
        if part.lower() in (".git", ".ag"):
            raise ValueError(f"invalid path {path!r}: {part} is reserved")
    if _CONTROL.search(path):
        raise ValueError(f"invalid path {path!r}: control character or backslash")


def build_changeset(
    *,
    namespace: str,
    author: str,
    timestamp_ms: int,
    ops: Sequence[PutOp | DeleteOp],
    parents: Sequence[str] = (),
    message: str = "",
) -> BuiltChangeset:
    """Encodes a ``ChangesetV1`` exactly as the object-format document specifies.

    Operations are sorted by path bytes for you. ``namespace`` is the repository's namespace
    (``Repo.namespace``, normally equal to its name); it is part of the hash.
    """
    if (
        not namespace
        or len(namespace) > 128
        or not all(_NAMESPACE_SEGMENT.match(s) for s in namespace.split("/"))
    ):
        raise ValueError(f"invalid namespace {namespace!r}")
    if not _IDENTITY.match(author):
        raise ValueError(f"invalid author identity {author!r}")
    if len(parents) > MAX_PARENTS:
        raise ValueError("too many parents")
    if len(set(parents)) != len(parents):
        raise ValueError("duplicate parent")
    for p in parents:
        if not _HEX64.match(p):
            raise ValueError(f"invalid parent id {p!r}")
    if len(message.encode("utf-8")) > MAX_MESSAGE_BYTES:
        raise ValueError("message too long")
    if len(ops) > MAX_OPS:
        raise ValueError("too many operations")

    blobs: dict[str, StoredObject] = {}
    encoded: list[tuple[bytes, bytes]] = []
    for op in ops:
        assert_repo_path(op.path)
        key = op.path.encode("utf-8")
        if isinstance(op, DeleteOp):
            encoded.append((key, b"\x02" + _string(op.path)))
            continue
        if op.executable and op.file_type == "symlink":
            raise ValueError(f"{op.path!r}: a symlink cannot be executable")
        blob = op.blob
        if op.content is not None:
            if blob is not None:
                raise ValueError(f"{op.path!r}: give either content or blob, not both")
            data = op.content.encode("utf-8") if isinstance(op.content, str) else bytes(op.content)
            blob = blob_id(data)
            if blob not in blobs:
                blobs[blob] = StoredObject(blob, encode_envelope(OBJECT_KIND_BLOB, data))
        if blob is None or not _HEX64.match(blob):
            raise ValueError(f"{op.path!r}: a put needs content or a blob id")
        encoded.append(
            (
                key,
                b"\x01"
                + _string(op.path)
                + bytes([1 if op.file_type == "symlink" else 0, 1 if op.executable else 0])
                + bytes.fromhex(blob),
            )
        )
    encoded.sort(key=lambda e: e[0])
    for a, b in pairwise(encoded):
        if a[0] == b[0]:
            raise ValueError("a path appears twice")

    payload = (
        struct.pack(">H", 1)
        + _string(namespace)
        + _varint(len(parents))
        + b"".join(bytes.fromhex(p) for p in parents)
        + _string(author)
        + struct.pack(">q", timestamp_ms)
        + _string(message)
        + _varint(len(encoded))
        + b"".join(e[1] for e in encoded)
    )
    cs_id = object_id(OBJECT_KIND_CHANGESET, OBJECT_VERSION, payload)
    return BuiltChangeset(
        id=cs_id,
        payload=payload,
        object=StoredObject(cs_id, encode_envelope(OBJECT_KIND_CHANGESET, payload)),
        blobs=tuple(blobs.values()),
    )
