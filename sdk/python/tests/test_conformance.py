"""The helpers must produce exactly the bytes of the independent reference implementations
(tests/conformance/gen_vectors.py and sdk/testdata/gen_signing_vectors.py). No server needed."""

from __future__ import annotations

import json
from typing import Any

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

from agentgit import (
    DeleteOp,
    PutOp,
    agent_key_from_seed,
    attestation_signing_bytes,
    blob_id,
    build_changeset,
    canonical_json,
    canonical_provenance_bytes,
    decode_base64,
    decode_envelope,
    encode_base64,
    encode_envelope,
    generate_agent_key,
    object_id,
    sign_attestation,
    sign_provenance,
    signing_message,
    verify_provenance,
)
from tests.harness import REPO_ROOT

VECTORS = json.loads((REPO_ROOT / "tests/conformance/vectors.json").read_text())
SIGNING = json.loads((REPO_ROOT / "sdk/testdata/signing-vectors.json").read_text())


@pytest.mark.parametrize("blob", VECTORS["blobs"], ids=lambda b: repr(b["content_utf8"]))
def test_blob_ids_match_the_vectors(blob: dict[str, str]) -> None:
    assert blob_id(blob["content_utf8"]) == blob["id"]
    assert blob_id(blob["content_utf8"].encode()) == blob["id"]


def test_kinds_and_versions_are_part_of_the_id() -> None:
    data = b"same bytes"
    assert object_id(1, 1, data) != object_id(2, 1, data)
    assert object_id(1, 1, data) != object_id(1, 2, data)


def test_the_object_envelope_round_trips() -> None:
    env = encode_envelope(1, b"hello\n")
    assert env[:7].hex() == "41474f42010001"
    assert decode_envelope(env).kind == 1
    with pytest.raises(ValueError):
        decode_envelope(b"nope")


@pytest.mark.parametrize("tree", VECTORS["trees"], ids=lambda t: t["name"])
def test_tree_ids_match_the_vectors(tree: dict[str, Any]) -> None:
    assert object_id(3, 1, bytes.fromhex(tree["payload_hex"])) == tree["id"]


def _ops(vector: dict[str, Any]) -> list[PutOp | DeleteOp]:
    ops: list[PutOp | DeleteOp] = []
    for o in vector["ops"]:
        if o["op"] == "delete":
            ops.append(DeleteOp(o["path"]))
        else:
            ops.append(
                PutOp(
                    o["path"],
                    content=o.get("content", ""),
                    file_type="symlink" if o.get("type") == "symlink" else "regular",
                    executable=bool(o.get("executable")),
                )
            )
    return ops


@pytest.mark.parametrize("vector", VECTORS["changesets"], ids=lambda v: v["name"])
def test_changesets_encode_byte_for_byte(vector: dict[str, Any]) -> None:
    built = build_changeset(
        namespace=vector["namespace"],
        parents=vector["parents"],
        author=vector["author"],
        timestamp_ms=vector["timestamp_ms"],
        message=vector["message"],
        ops=_ops(vector),
    )
    assert built.payload.hex() == vector["payload_hex"]
    assert built.id == vector["id"]
    assert built.object.envelope[7:].hex() == vector["payload_hex"]


def test_blobs_are_collected_once_and_ops_are_sorted() -> None:
    built = build_changeset(
        namespace="acme/web",
        author="user:alice",
        timestamp_ms=1,
        ops=[PutOp("a.txt", content="same"), PutOp("b.txt", content="same"), PutOp("c.txt", content="other")],
    )
    assert sorted(b.id for b in built.blobs) == sorted([blob_id("same"), blob_id("other")])
    assert [o.id for o in built.objects()][-1] == built.id
    a = build_changeset(
        namespace="acme/web", author="user:alice", timestamp_ms=1,
        ops=[PutOp("b", content="1"), PutOp("a/z", content="2"), DeleteOp("a/a")],
    )  # fmt: skip
    b = build_changeset(
        namespace="acme/web", author="user:alice", timestamp_ms=1,
        ops=[DeleteOp("a/a"), PutOp("a/z", content="2"), PutOp("b", content="1")],
    )  # fmt: skip
    assert a.id == b.id


@pytest.mark.parametrize(
    ("kwargs", "message"),
    [
        ({"ops": [PutOp("../etc/passwd", content="x")]}, "invalid path"),
        ({"ops": [PutOp("/abs", content="x")]}, "relative"),
        ({"ops": [PutOp("a/.GIT/config", content="x")]}, "reserved"),
        ({"ops": [PutOp("a\\b", content="x")]}, "backslash"),
        ({"ops": [PutOp("a", content="x"), PutOp("a", content="y")]}, "twice"),
        ({"ops": [], "author": "bob"}, "identity"),
        ({"ops": [], "namespace": "Acme/Web"}, "namespace"),
        ({"ops": [], "parents": ["zz"]}, "parent"),
        ({"ops": [PutOp("l", content="x", file_type="symlink", executable=True)]}, "symlink"),
        ({"ops": [PutOp("a")]}, "needs content"),
    ],
)
def test_changesets_reject_what_the_format_forbids(kwargs: dict[str, Any], message: str) -> None:
    args: dict[str, Any] = {"namespace": "acme/web", "author": "user:alice", "timestamp_ms": 1}
    args.update(kwargs)
    with pytest.raises(ValueError, match=message):
        build_changeset(**args)


@pytest.mark.parametrize("att", VECTORS["attestations"], ids=lambda a: a["result"])
def test_attestation_signing_bytes_match_the_vectors(att: dict[str, Any]) -> None:
    assert attestation_signing_bytes(att).hex() == att["signing_bytes_hex"]


def test_attestation_signatures_verify_with_an_independent_implementation() -> None:
    key = generate_agent_key()
    att = VECTORS["attestations"][0]
    signed = sign_attestation({k: v for k, v in att.items() if k != "signing_bytes_hex"}, key)
    message = signing_message("attestation.v1", bytes.fromhex(att["signing_bytes_hex"]))
    Ed25519PublicKey.from_public_bytes(key.public_key).verify(bytes.fromhex(signed["signature"]), message)
    assert signed["public_key"] == key.public_key_hex


@pytest.fixture(scope="module")
def key():
    return agent_key_from_seed(SIGNING["seed_hex"])


def test_the_public_key_of_the_vector_seed(key) -> None:  # type: ignore[no-untyped-def]
    assert key.public_key_hex == SIGNING["public_key_hex"]


@pytest.mark.parametrize("case", SIGNING["cases"], ids=lambda c: c["name"])
def test_provenance_canonical_bytes_signed_message_and_signature(case: dict[str, Any], key) -> None:  # type: ignore[no-untyped-def]
    record = case["record"]
    assert canonical_provenance_bytes(record).decode() == case["canonical_json"]
    assert (
        signing_message("provenance.v1", canonical_provenance_bytes(record)).hex()
        == case["signed_message_hex"]
    )
    # Ed25519 is deterministic: the signature equals the one OpenSSL made in the generator.
    assert sign_provenance(record, key.private_key) == case["signature_hex"]
    assert verify_provenance(record, case["signature_hex"], key.public_key)


def test_signatures_agree_with_openssl_for_random_keys() -> None:
    record = SIGNING["cases"][1]["record"]
    for _ in range(5):
        k = generate_agent_key()
        ours = sign_provenance(record, k.private_key)
        message = signing_message("provenance.v1", canonical_provenance_bytes(record))
        Ed25519PublicKey.from_public_bytes(k.public_key).verify(bytes.fromhex(ours), message)
        assert Ed25519PrivateKey.from_private_bytes(k.private_key).sign(message).hex() == ours


def test_tampered_records_foreign_keys_and_flipped_bits_are_rejected(key) -> None:  # type: ignore[no-untyped-def]
    record = dict(SIGNING["cases"][0]["record"])
    sig = sign_provenance(record, key.private_key)
    assert not verify_provenance({**record, "model": "other"}, sig, key.public_key)
    assert not verify_provenance(record, sig, generate_agent_key().public_key)
    flipped = sig[:-1] + ("1" if sig.endswith("0") else "0")
    assert not verify_provenance(record, flipped, key.public_key)
    assert not verify_provenance(record, "not hex", key.public_key)


def test_signatures_are_domain_separated(key) -> None:  # type: ignore[no-untyped-def]
    record = SIGNING["cases"][0]["record"]
    sig = bytes.fromhex(sign_provenance(record, key.private_key))
    body = canonical_provenance_bytes(record)
    pub = Ed25519PublicKey.from_public_bytes(key.public_key)
    from cryptography.exceptions import InvalidSignature

    for wrong in (signing_message("attestation.v1", body), body):
        with pytest.raises(InvalidSignature):
            pub.verify(sig, wrong)


def test_canonical_json_sorts_keys_at_every_level_and_refuses_floats() -> None:
    assert (
        canonical_json({"b": 1, "a": {"d": [3, {"y": 1, "x": 2}], "c": None}})
        == '{"a":{"c":null,"d":[3,{"x":2,"y":1}]},"b":1}'
    )
    with pytest.raises(TypeError):
        canonical_json({"n": 1.5})


def test_base64_round_trips_and_matches_the_standard_library() -> None:
    import base64

    for n in range(40):
        data = bytes((i * 37 + n) & 0xFF for i in range(n))
        text = encode_base64(data)
        assert text == base64.b64encode(data).decode()
        assert decode_base64(text) == data
    assert decode_base64("aGk") == b"hi"  # padding optional
    with pytest.raises(ValueError):
        decode_base64("a$b=")
    with pytest.raises(ValueError):
        decode_base64("a")
