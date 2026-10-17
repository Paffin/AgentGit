#!/usr/bin/env python3
"""Computes expected.json for a compatibility fixture - with an implementation that is NOT the Rust one.

generate.sh has just built the repository with the current `ag`. This script reads it back through the CLI and
then re-derives every identifier from first principles with the independent Python encoder from
tests/conformance/gen_vectors.py:

  * each changeset id, from its author, time, message, parents and the operations that turn its first parent's
    tree into its own;
  * each tree id, from the file listing;
  * the audit hash chain, entry by entry.

If the Rust code and this script disagree the fixture is not written. What lands in expected.json is therefore
a golden that a future version of AgentGit has to reproduce, and that has been cross-checked against the
specification once.

  expected.py <dir with captured --json outputs> <ag> <repository>   > expected.json
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("gen_vectors", HERE.parent / "conformance" / "gen_vectors.py")
gv = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gv)

JSON_DIR, AG, REPO = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
ENV = {**os.environ, "AG_IDENTITY": "user:alice", "NO_COLOR": "1"}


def ag(*args: str):
    out = subprocess.run([AG, "-C", REPO, "--json", *args], capture_output=True, text=True, env=ENV)
    if out.returncode != 0:
        sys.exit(f"ag {' '.join(args)} failed: {out.stderr}")
    return json.loads(out.stdout)


def captured(name: str):
    return json.loads((JSON_DIR / f"{name}.json").read_text())


def walk(rev: str, base: str = "") -> dict[str, dict]:
    """Every file and symlink of a revision: path -> {content, type, executable}."""
    files: dict[str, dict] = {}
    for e in ag("ls", rev, base) if base else ag("ls", rev):
        path = f"{base}/{e['name']}" if base else e["name"]
        if e["type"] == "dir":
            files.update(walk(rev, path))
        else:
            files[path] = {"content": e["blob"], "type": e["type"], "executable": bool(e.get("executable"))}
    return files


def tree_id(files: dict[str, dict]) -> str:
    payload = bytearray(gv.varint(len(files)))
    for path in sorted(files, key=lambda p: p.encode()):
        e = files[path]
        payload += gv.s(path) + bytes([0 if e["type"] == "file" else 1, 1 if e["executable"] else 0])
        payload += bytes.fromhex(e["content"])
    return gv.object_id(3, 1, bytes(payload))


def changeset_id(namespace, parents, author, ts, message, before, after) -> str:
    ops = []
    for path in sorted(set(before) | set(after), key=lambda p: p.encode()):
        b, a = before.get(path), after.get(path)
        if a == b:
            continue
        ops.append((path, a))
    out = bytearray((1).to_bytes(2, "big"))
    out += gv.s(namespace) + gv.varint(len(parents))
    for p in parents:
        out += bytes.fromhex(p)
    out += gv.s(author) + ts.to_bytes(8, "big", signed=True) + gv.s(message) + gv.varint(len(ops))
    for path, a in ops:
        if a is None:
            out += bytes([2]) + gv.s(path)
        else:
            out += bytes([1]) + gv.s(path) + bytes([0 if a["type"] == "file" else 1, 1 if a["executable"] else 0])
            out += bytes.fromhex(a["content"])
    return gv.object_id(2, 1, bytes(out))


namespace = "acme/compat"
log = captured("log")  # newest first
by_id = {c["id"]: c for c in log}
trees: dict[str, dict] = {}
for c in log:
    trees[c["id"]] = walk(c["id"])

changesets = []
for c in sorted(log, key=lambda c: (c["generation"], c["id"])):
    before = trees[c["parents"][0]] if c["parents"] else {}
    expected_id = changeset_id(namespace, c["parents"], c["author"], c["timestamp"], c["message"], before, trees[c["id"]])
    if expected_id != c["id"]:
        sys.exit(f"changeset {c['id']} is {expected_id} according to the reference encoder: formats disagree")
    changesets.append(
        {
            "id": c["id"],
            "generation": c["generation"],
            "parents": c["parents"],
            "author": c["author"],
            "timestamp_ms": c["timestamp"],
            "message": c["message"],
            "tree_id": tree_id(trees[c["id"]]),
            "files": len(trees[c["id"]]),
        }
    )

# --- audit chain ---------------------------------------------------------------------------------------------
entries = sorted(captured("audit_list"), key=lambda e: e["seq"])
prev = "0" * 64
for e in entries:
    details = json.dumps(json.loads(e["details"]), separators=(",", ":"), sort_keys=True, ensure_ascii=False)
    if details != e["details"]:
        sys.exit(f"audit entry {e['seq']}: details are not in canonical form")
    pre = gv.s("agentgit.audit.v1") + gv.s(prev) + e["timestamp"].to_bytes(8, "big", signed=True)
    pre += gv.s(e["actor"]) + gv.s(e["action"]) + gv.s(e["target"]) + gv.s(details)
    h = hashlib.sha256(pre).hexdigest()
    if h != e["hash"] or e["prev_hash"] != prev:
        sys.exit(f"audit entry {e['seq']} does not match the reference chain")
    prev = h
verify = captured("verify")
assert verify["issues"] == [] and verify["audit_head"] == prev and verify["audit_entries"] == len(entries)

# --- attestations, per tree ------------------------------------------------------------------------------------
attestations = []
for c in changesets:
    for a in ag("validate", "list", c["tree_id"]):
        attestations.append({"tree_id": c["tree_id"], "changeset": c["id"], **a})

# --- the rest, as the CLI printed it -------------------------------------------------------------------------------
policy = captured("policy_history")
head = next(s for s in captured("streams") if s["name"] == "main")["state"]["head"]
expected = {
    "format": "agentgit-compat-expected-v1",
    "namespace": namespace,
    "streams": {s["name"]: s["state"] for s in captured("streams")},
    "head": head,
    "changesets": changesets,
    "merge_changesets": [
        c["id"] for c in changesets if len(c["parents"]) == 2 and "Merge-Algorithm:" in c["message"]
    ],
    "files_at_head": trees[head],
    "renames_at_head": [c for c in captured("show_head")["changes"] if c["op"] == "R"],
    "audit": {"entries": len(entries), "head": prev, "first_actions": [e["action"] for e in entries[:6]]},
    "policy_versions": [
        {
            "version": i + 1,
            "created_by": p["created_by"],
            "doc_sha256": hashlib.sha256(p["doc"].encode()).hexdigest(),
        }
        for i, p in enumerate(sorted(policy, key=lambda p: p.get("version", p["created_at"])))
    ],
    "attestations": attestations,
    "blame": {
        name: [{"text": l["text"], "changeset": l["changeset"], "path": l["path"], "line": l["line"]} for l in captured(f"blame_{name}")["lines"]]
        for name in ("main", "helpers")
    },
    "pins": captured("pins"),
    "agents": {"runs": captured("runs"), "provenance": captured("provenance")},
    "tasks": [{"id": t["id"], "title": t["title"], "state": t["state"]} for t in captured("tasks")],
    "leases": [{"id": l["id"], "scope": l["scope"], "exclusive": l["exclusive"], "token": l["token"]} for l in captured("leases")],
}
print(json.dumps(expected, indent=1, sort_keys=True))
