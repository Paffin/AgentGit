#!/usr/bin/env python3
"""Reference vectors for provenance signing, produced WITHOUT either SDK.

Only the standard library's `json` and the `cryptography` package (OpenSSL's Ed25519) are used, so
agreement with the TypeScript and Python SDKs is evidence about the format, not about shared code.

    python3 sdk/testdata/gen_signing_vectors.py > sdk/testdata/signing-vectors.json

The format (docs/protocol/agwp-v1.md, "Signing formats"):

    message = "agentgit.sig.v1\\0" || len(domain):u8 || domain || payload        domain "provenance.v1"
    payload = the record as compact JSON, keys sorted at every level, unset optionals as null
"""

import json
import sys

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

SEED = bytes(range(1, 33))


def canonical(record: dict) -> str:
    def opt(key: str):
        value = record.get(key)
        return value if value else None

    body = {
        "schema": "agentgit.provenance.v1",
        "changeset": record["changeset"],
        "agent": record["agent"],
        "run_id": opt("run_id"),
        "task_id": opt("task_id"),
        "model": opt("model"),
        "prompt_digest": opt("prompt_digest"),
        "parameters": record.get("parameters", {}),
        "checks_run": record.get("checks_run", []),
        "inputs": record.get("inputs", []),
        "rationale": opt("rationale"),
        "created_at": record["created_at_ms"],
    }
    return json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def signed_message(domain: str, payload: bytes) -> bytes:
    d = domain.encode()
    return b"agentgit.sig.v1\0" + bytes([len(d)]) + d + payload


CHANGESET = "ab" * 32

CASES = [
    ("minimal", {"changeset": CHANGESET, "agent": "agent:bot", "created_at_ms": 1760659200000}),
    (
        "full-with-unsorted-parameters",
        {
            "changeset": CHANGESET,
            "agent": "agent:refactor-bot#run_0123456789abcdef0123",
            "run_id": "run_0123456789abcdef0123",
            "task_id": "task_0123",
            "model": "example-model-1",
            "prompt_digest": "0" * 63 + "1",
            "parameters": {"temperature": "0.2", "max_tokens": "4096", "a-first": "x"},
            "checks_run": ["lint", "unit"],
            "inputs": ["https://example.org/issue/1", "docs/guide.md"],
            "rationale": "rename for clarity",
            "created_at_ms": 1760659260123,
        },
    ),
    (
        "escapes-and-unicode",
        {
            "changeset": CHANGESET,
            "agent": "agent:bot#run_1",
            "rationale": 'quote " backslash \\ newline \n tab \t control \u0001 del \u007f snowman ☃ crab \U0001f980',
            "inputs": ["Привет, мир", "line\r\nbreak", "  separator"],
            "parameters": {"ключ": "значение", "kéy": "v"},
            "created_at_ms": -1,
        },
    ),
    (
        "key-order-is-by-code-point-not-utf16",
        {
            "changeset": CHANGESET,
            "agent": "agent:bot",
            # U+FFFF sorts after U+10000 in UTF-16 code units but before it by code point / UTF-8 bytes.
            "parameters": {"\U00010000": "astral", "￿": "bmp-last", "z": "ascii", "é": "latin"},
            "created_at_ms": 0,
        },
    ),
    (
        "empty-strings-are-unset",
        {
            "changeset": CHANGESET,
            "agent": "agent:bot",
            "run_id": "",
            "model": "",
            "rationale": "",
            "created_at_ms": 7,
        },
    ),
]


def main() -> None:
    key = Ed25519PrivateKey.from_private_bytes(SEED)
    public = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    cases = []
    for name, record in CASES:
        text = canonical(record)
        message = signed_message("provenance.v1", text.encode("utf-8"))
        cases.append(
            {
                "name": name,
                "record": record,
                "canonical_json": text,
                "signed_message_hex": message.hex(),
                "signature_hex": key.sign(message).hex(),
            }
        )
    json.dump(
        {"format": "agentgit-sdk-signing-vectors-v1", "seed_hex": SEED.hex(), "public_key_hex": public.hex(), "cases": cases},
        sys.stdout,
        indent=2,
        ensure_ascii=False,
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
