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


def tree_manifest(entries) -> bytes:
    """entries: list of dicts {path, content, type?, executable?}; sorted bytewise by path."""
    entries = sorted(entries, key=lambda e: e["path"].encode("utf-8"))
    out = bytearray(varint(len(entries)))
    for e in entries:
        out += s(e["path"])
        out += bytes([0 if e.get("type", "file") == "file" else 1])
        out += bytes([1 if e.get("executable") else 0])
        out += bytes.fromhex(blob_id(e["content"].encode("utf-8")))
    return bytes(out)


def attestation_signing_bytes(a) -> bytes:
    out = bytearray()
    out += s(a["id"])
    out += bytes.fromhex(a["tree_id"])
    out += s(a["check"])
    out += bytes([1 if a["result"] == "pass" else 2])
    out += varint(a["policy_version"])
    out += s(a["environment_digest"])
    out += s(a["issuer"])
    out += a["issued_at_ms"].to_bytes(8, "big", signed=True)
    return bytes(out)


def audit_chain(entries):
    prev = "0" * 64
    rows = []
    for e in entries:
        details = json.dumps(e["details"], separators=(",", ":"), sort_keys=True, ensure_ascii=False)
        pre = s("agentgit.audit.v1") + s(prev) + e["ts"].to_bytes(8, "big", signed=True)
        pre += s(e["actor"]) + s(e["action"]) + s(e["target"]) + s(details)
        h = hashlib.sha256(pre).hexdigest()
        rows.append({**e, "prev_hash": prev, "hash": h})
        prev = h
    return rows


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
    tree_entries = [
        {"path": "src/main.rs", "content": "fn main() {}\n"},
        {"path": "bin/run.sh", "content": "#!/bin/sh\n", "executable": True},
        {"path": "link", "content": "src/main.rs", "type": "symlink"},
        {"path": "файл.txt", "content": ""},
    ]
    trees = [
        {"name": "empty", "entries": [], "payload_hex": tree_manifest([]).hex(), "id": object_id(3, 1, tree_manifest([]))},
        {"name": "mixed", "entries": tree_entries, "payload_hex": tree_manifest(tree_entries).hex(),
         "id": object_id(3, 1, tree_manifest(tree_entries))},
    ]
    att = {"id": "att_0123456789abcdef", "tree_id": trees[1]["id"], "check": "tests", "result": "pass",
           "policy_version": 3, "environment_digest": "sha256:abc", "issuer": "service:ci",
           "issued_at_ms": 1760659200000}
    attestations = [{**att, "signing_bytes_hex": attestation_signing_bytes(att).hex()},
                    {**att, "result": "fail", "policy_version": 300,
                     "signing_bytes_hex": attestation_signing_bytes({**att, "result": "fail", "policy_version": 300}).hex()}]
    audit = audit_chain([
        {"ts": 1000, "actor": "user:alice", "action": "repo.init", "target": "acme/app", "details": {"default_stream": "main"}},
        {"ts": 2000, "actor": "agent:bot#run_1", "action": "stream.integrate", "target": "main",
         "details": {"version": 1, "proposal": "prop_x", "unicode": "юникод"}},
        {"ts": 3000, "actor": "service:ci", "action": "attestation.submit", "target": trees[1]["id"], "details": {}},
    ])
    json.dump({"format": "agentgit-conformance-v1", "blobs": blobs, "changesets": vectors, "trees": trees,
               "attestations": attestations, "audit": audit},
              sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
