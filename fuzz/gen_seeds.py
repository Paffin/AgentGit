#!/usr/bin/env python3
"""Writes the (small) seed corpora under fuzz/corpus/<target>/.

Seeds are what the fuzzer starts from: valid inputs reach deep code immediately instead of spending the first
minutes rediscovering the magic bytes. The binary formats come from the independent Python implementation's
vectors (tests/conformance/vectors.json); the rest are hand-written examples of what each parser is for.
The corpora are checked in, so this script only needs to run when a target or a format is added.

  fuzz/gen_seeds.py
"""
import json
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = ROOT / "fuzz" / "corpus"
VECTORS = json.loads((ROOT / "tests" / "conformance" / "vectors.json").read_text())

counts = {}


def put(target: str, data: bytes, name: str | None = None) -> None:
    d = CORPUS / target
    d.mkdir(parents=True, exist_ok=True)
    n = counts.get(target, 0)
    counts[target] = n + 1
    (d / (name or f"seed-{n:02d}")).write_bytes(data)


def envelope(kind: int, version: int, payload: bytes) -> bytes:
    return b"AGOB" + bytes([kind]) + struct.pack(">H", version) + payload


# --- binary formats ------------------------------------------------------------------------------------------
for cs in VECTORS["changesets"]:
    payload = bytes.fromhex(cs["payload_hex"])
    put("changeset_decode", payload)
    put("object_envelope", envelope(2, 1, payload))
for tr in VECTORS["trees"]:
    payload = bytes.fromhex(tr["payload_hex"])
    put("tree_decode", payload)
    put("object_envelope", envelope(3, 1, payload))
for b in VECTORS["blobs"][:2]:
    put("object_envelope", envelope(1, 1, b["content_utf8"].encode()))
put("object_envelope", b"AGOB\x01\x00\x01")  # an empty blob
put("object_envelope", b"AGOB\x09\x00\x01x")  # unknown kind
put("object_envelope", b"AG")  # truncated

# varints: 0, 127, 128, a non-minimal one, an overflowing one; then length-prefixed strings
put("canon_reader", bytes([0b00000001, 0x00, 0x7F]))
put("canon_reader", bytes([0b00000001, 0x80, 0x01, 0x80, 0x00]))
put("canon_reader", bytes([0b00000010, 0x03]) + b"abc")
put("canon_reader", bytes([0b00000011]) + b"\xff" * 10)
put("canon_reader", bytes([0b00000010, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]) + b"xx")

# --- paths, names ------------------------------------------------------------------------------------------
for s in [
    "src/lib.rs", "a/b/c.txt", ".github/workflows/ci.yml", "README.md", "dir with space/f", "\"quoted\"",
    "../etc/passwd", "a/../b", "/abs", "a//b", ".git/config", "a/.GIT/x", ".ag", "a\\b", "a\x00b", "a\nb",
    "user:alice", "agent:bot#run_1", "service:ci", "system:root", "acme/imported", "feature/x-1.2",
    "0" * 64, "e" * 64, "main", "op_12345678", "é/日本語.txt", "a" * 300,
]:
    put("repo_path", s.encode())
put("repo_path", struct.pack(">q", -(2**63)) + b"x")
put("repo_path", struct.pack(">q", 2**63 - 1) + b"x")

# --- diff and merge: inputs separated by 0xff ---------------------------------------------------------------
def text(*lines: str, nl: bool = True) -> bytes:
    return ("\n".join(lines) + ("\n" if nl else "")).encode()


base = text("a", "b", "c", "d", "e", "f", "g", "h")
put("diff_myers", bytes([2]) + base + b"\xff" + text("a", "B", "c", "d", "e", "f", "G", "h"))
put("diff_myers", bytes([0]) + base + b"\xff" + text("h", "g", "f", "e", "d", "c", "b", "a"))
put("diff_myers", bytes([3]) + text("x", nl=False) + b"\xff" + text("x"))
put("diff_myers", bytes([4]) + b"\xff" + text("new"))
put("diff_myers", bytes([4]) + b"\x00bin\xff\x00bin2")
put("merge_text", base + b"\xff" + text("A", "b", "c", "d", "e", "f", "g", "h") + b"\xff" + text("a", "b", "c", "d", "e", "f", "g", "H"))
put("merge_text", base + b"\xff" + text("a", "X", "c", "d", "e", "f", "g", "h") + b"\xff" + text("a", "Y", "c", "d", "e", "f", "g", "h"))
put("merge_text", text("a", "b") + b"\xff" + text("a", "N", "b") + b"\xff" + text("a", "M", "b"))
put("merge_text", text("a", "b", nl=False) + b"\xff" + text("a", "b") + b"\xff" + text("a", "b", nl=False))
put("merge_text", b"\xff\xff")
# generators take any bytes; a few fixed ones give the first trees something to chew on
for i in range(4):
    for target in ("merge_trees", "tree_ops", "rename_detect"):
        put(target, bytes((i * 37 + j * 11 + len(target)) & 0xFF for j in range(160)))

# --- policy -------------------------------------------------------------------------------------------------
PUB = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"  # RFC 8032 test key 1
policies = [
    "{}",
    '{"approvals":{"min":1}}',
    '{"require_resolved_threads":true}',
    '{"integrators":["user:*"],"allowed_authors":["user:*","agent:*"],"max_changed_files":50}',
    json.dumps({
        "integrators": ["user:*"],
        "trusted_issuers": [{"identity": "service:ci", "public_key": PUB}],
        "required_validations": [{
            "check": "tests", "issuers": ["service:ci"], "max_age_secs": 3600,
            "run": {"argv": ["cargo", "test"], "timeout_secs": 600, "env": {"CI": "1"}, "network": False},
        }],
        "approvals": {"min": 1, "min_for_agents": 2, "approvers": ["user:*"], "forbid_self_approval": True},
        "protected_paths": [{"glob": ".github/**", "min_approvals": 2, "approvers": ["user:alice"]}],
        "agent_autonomy": {"auto_integrate": True, "allowed_paths": ["docs/**", "**/*.md"], "max_files": 5},
        "require_provenance_for_agents": True,
        "max_changed_files": 100,
    }),
    '{"protected_paths":[{"glob":"[","min_approvals":1}]}',
    '{"approvals":{"mn":2}}',
]
for p in policies:
    put("policy_json", p.encode())
for i in range(4):
    put("policy_eval", bytes((i * 53 + j * 7) & 0xFF for j in range(96)))

# --- globs --------------------------------------------------------------------------------------------------
for s in [
    "src/**\n**/*.rs\n*.md\n\0\0src/lib.rs\ndocs/a/b.md\nREADME.md",
    "{a,b}/**\n[ab]*/x\n?.rs\n\0\0a/x\nb/c/d\nz.rs",
    "{a,{b,{c,{d,{e,f}}}}}\n**/**/**/**/*\n\0\0q/w/e/r/t",
    "user:*\nagent:bot*\n\0\0x",
]:
    put("glob_match", s.encode())
for s in [
    "target/\n*.log\n/build\n# comment\n\n\0\0target/x\nsrc/a.log\nbuild/out",
    "**/node_modules\n!keep\ndir/*/x\n\0\0a/node_modules/b\ndir/y/x",
]:
    put("ignore_rules", s.encode())

# --- JSON of signed things ----------------------------------------------------------------------------------
sig = "00" * 64
put("signed_json", json.dumps({
    "id": "att_0123456789abcdef", "tree_id": "20" * 32, "check": "tests", "result": "pass", "policy_version": 3,
    "environment_digest": "sha256:abc", "issuer": "service:ci", "issued_at": 1760659200000,
    "public_key": PUB, "signature": sig,
}).encode())
put("signed_json", json.dumps({
    "schema": "agentgit.provenance.v1", "changeset": "ab" * 32, "agent": "agent:bot#run_1", "run_id": "run_1",
    "task_id": None, "model": "model-x", "prompt_digest": "cd" * 32, "parameters": {"temperature": "0"},
    "checks_run": ["cargo test"], "inputs": ["https://example.com"], "rationale": "fix", "created_at": 1760659200000,
}).encode())
put("signed_json", (PUB + sig).encode())
# regression: the hex-split harness once cut a multi-byte character in half (length 3 -> midpoint inside it)
put("signed_json", "\u20ac".encode(), name="regress-split-multibyte")
# seed (32 bytes) + message for the real-signature check
put("signed_json", bytes(range(32)) + b"signed message", name="seed-and-message")

# --- sandbox and jobs ---------------------------------------------------------------------------------------
put("sandbox_spec", json.dumps({
    "argv": ["sh", "-c", "true"], "env": {"CI": "true"}, "workdir": "/tmp",
    "ro_mounts": [{"host": "/usr", "sandbox": "/usr", "identity": "usr@1"}], "rw_tmp_bytes": 1048576,
    "limits": {"wall_ms": 1000, "cpu_s": 1, "memory_bytes": 67108864, "pids": 16},
    "network": "none", "stdin": [104, 105],
}).encode())
put("sandbox_spec", json.dumps({
    "argv": ["x"], "env": {}, "workdir": "/tmp", "ro_mounts": [{"host": "/", "sandbox": "/work/x"}],
    "rw_tmp_bytes": 1, "limits": {}, "network": "host", "stdin": [],
}).encode())


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def field(num: int, wire: int, payload: bytes) -> bytes:
    return varint(num << 3 | wire) + payload


def ld(num: int, data: bytes) -> bytes:
    return field(num, 2, varint(len(data)) + data)


run = ld(1, b"cargo") + ld(1, b"test") + field(2, 0, varint(600)) + field(3, 0, varint(2048)) + ld(8, ld(1, b"CI") + ld(2, b"1"))
task = ld(2, b"tests") + ld(3, b"20" * 32) + field(4, 0, varint(3)) + ld(6, run)
put("job_spec", task)
put("job_spec", ld(6, ld(1, b"sh") + field(6, 0, varint(1)) + ld(7, b"img@sha256:" + b"0" * 64)))
put("job_spec", ld(6, ld(1, b"x") + field(2, 0, varint(10**9))))

# --- AGWP: two bytes of message index, then a body ----------------------------------------------------------
for idx, body in [
    (0, b'{}'), (1, b'{"client":"fuzz","versions":["agwp.v1"]}'), (6, b'{"name":"acme/x","description":"d"}'),
    (20, b'{"repo":"acme/x"}'), (40, b'{"repo":"acme/x","stream":"main","expected_head":"","new_head":"00"}'),
    (100, b'{"repo":"r","payload":{"a":[1,2,{"b":null}]}}'), (150, b'{"data":"AAEC","content":"AAEC"}'),
]:
    put("agwp_json", struct.pack("<H", idx) + body)
    put("agwp_proto", struct.pack("<H", idx) + ld(1, b"acme/x") + ld(2, b"main") + field(3, 0, varint(5)))
put("agwp_proto", struct.pack("<H", 12) + ld(1, b"\x00" * 100))
put("agwp_proto", struct.pack("<H", 12) + b"\x0a\xff\xff\xff\xff\x0f")  # a declared length far past the end

# --- Git bridge ----------------------------------------------------------------------------------------------
raw = (b":100644 100644 " + b"a" * 40 + b" " + b"b" * 40 + b" M\0src/lib.rs\0"
       + b":000000 100755 " + b"0" * 40 + b" " + b"c" * 40 + b" A\0bin/run.sh\0"
       + b":100644 000000 " + b"d" * 40 + b" " + b"0" * 40 + b" D\0old.txt\0"
       + b":160000 160000 " + b"e" * 40 + b" " + b"f" * 40 + b" M\0vendor/sub\0"
       + b":000000 120000 " + b"0" * 40 + b" " + b"9" * 40 + b" A\0link\0"
       + b":000000 100644 " + b"0" * 40 + b" " + b"8" * 40 + b" A\0\xff\xfe-bad-utf8\0")
put("git_parse", b"\x00" + raw)
def log_record(*fields: bytes) -> bytes:
    """One commit as `git log -z --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%B` prints it: NUL-terminated fields."""
    return b"".join(f + b"\0" for f in fields)


log = (log_record(b"1" * 40, b"", b"Ada Lovelace", b"ada@example.com", b"1700000000", b"subject\n\nbody\n")
       + log_record(b"2" * 40, b"1" * 40 + b" " + b"3" * 40, b"Bob", b"bob@example.com", b"9999999999999999", b"merge\n"))
put("git_parse", b"\x01" + log)
put("git_parse", b"\x01" + log_record(b"4" * 40, b"", b"n", b"e", b"-5", "я".encode() * 100))
put("git_parse", b"\x01" + log_record(b"5" * 40, b"", b"\xff\xfe", b"e", b"1", b"caf\xe9"))  # not UTF-8
put("git_parse", b"\x01a\0b")

print({k: v for k, v in sorted(counts.items())})
print("total bytes:", sum(p.stat().st_size for p in CORPUS.rglob("*") if p.is_file()))
