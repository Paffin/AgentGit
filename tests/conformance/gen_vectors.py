#!/usr/bin/env python3
"""Independent reference encoder for AgentGit object formats.

This script deliberately shares no code with the Rust implementation. It
re-implements the canonical encoding described in
docs/architecture/object-format.md and writes `vectors.json`, which the Rust
test-suite (`crates/ag-changeset/tests/conformance.rs`) checks byte for byte.

Usage: python3 tests/conformance/gen_vectors.py > tests/conformance/vectors.json
"""
import hashlib
import json
import sys


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n == 0:
            out.append(b)
            return bytes(out)
        out.append(b | 0x80)


def s(text: str) -> bytes:
    raw = text.encode("utf-8")
    return varint(len(raw)) + raw


def object_id(kind: int, version: int, payload: bytes) -> str:
    pre = (
        b"agentgit.object\0"
        + bytes([kind])
        + version.to_bytes(2, "big")
        + len(payload).to_bytes(8, "big")
        + payload
    )
    return hashlib.sha256(pre).hexdigest()


def blob_id(content: bytes) -> str:
    return object_id(1, 1, content)


def encode_changeset(ns, parents, author, ts_ms, message, ops) -> bytes:
    ops = sorted(ops, key=lambda o: o["path"].encode("utf-8"))
    out = bytearray()
    out += (1).to_bytes(2, "big")
    out += s(ns)
    out += varint(len(parents))
    for p in parents:
        out += bytes.fromhex(p)
    out += s(author)
    out += ts_ms.to_bytes(8, "big", signed=True)
    out += s(message)
    out += varint(len(ops))
    for op in ops:
        if op["op"] == "put":
            out += bytes([1]) + s(op["path"])
            out += bytes([0 if op.get("type", "file") == "file" else 1])
            out += bytes([1 if op.get("executable") else 0])
            out += bytes.fromhex(blob_id(op["content"].encode("utf-8")))
        else:
            out += bytes([2]) + s(op["path"])
    return bytes(out)


def vector(name, ns, parents, author, ts_ms, message, ops):
    payload = encode_changeset(ns, parents, author, ts_ms, message, ops)
    return {
        "name": name,
        "namespace": ns,
        "parents": parents,
        "author": author,
        "timestamp_ms": ts_ms,
        "message": message,
        "ops": ops,
        "payload_hex": payload.hex(),
        "id": object_id(2, 1, payload),
    }


def main():
    p1, p2 = "01" * 32, "02" * 32
    vectors = [
        vector("root-empty", "acme/app", [], "user:alice", 0, "", []),
        vector(
            "merge-with-mixed-ops",
            "acme/app",
            [p1, p2],
            "agent:bot#run-1",
            1760659200000,
            "add things",
            [
                {"op": "put", "path": "b.txt", "content": "b"},
                {"op": "delete", "path": "a/old.txt"},
                {"op": "put", "path": "a/new.txt", "content": "new"},
            ],
        ),
        vector(
            "unicode-and-modes",
            "org/repo.name_1",
            [p1],
            "service:ci",
            -1,
            "Привет, мир ☃",
            [
                {"op": "put", "path": "bin/run.sh", "content": "#!/bin/sh\n", "executable": True},
                {"op": "put", "path": "ссылка", "content": "target", "type": "symlink"},
                {"op": "put", "path": "файл.txt", "content": ""},
            ],
        ),
    ]
    blobs = [
        {"content_utf8": c, "id": blob_id(c.encode("utf-8"))}
        for c in ["", "hello\n", "new", "b"]
    ]
    json.dump({"format": "agentgit-conformance-v1", "blobs": blobs, "changesets": vectors},
              sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
