"""Content ids, provenance signatures and client-side changesets need no server at all.

python examples/signing.py
"""

# region signing
from agentgit import PutOp, blob_id, build_changeset, generate_agent_key, sign_provenance, verify_provenance

# The content id of a file: SHA-256 over "agentgit.object\0" || kind || version || length || bytes.
print(blob_id("hello\n"))  # 6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545

# Agents sign provenance records with their own Ed25519 key. The signature covers the record as
# compact JSON with sorted keys, under the domain "provenance.v1" - exactly what the server verifies.
key = generate_agent_key()
record = {
    "changeset": "ab" * 32,
    "agent": "agent:docs-bot#run_0123456789abcdef0123",
    "run_id": "run_0123456789abcdef0123",
    "model": "example-model",
    "created_at_ms": 1_760_659_200_000,
}
signature = sign_provenance(record, key.private_key)
print(verify_provenance(record, signature, key.public_key))  # True
print(verify_provenance({**record, "model": "other"}, signature, key.public_key))  # False

# A client that has no use for virtual workspaces can encode ChangesetV1 itself. The id is the same
# one the server derives from the uploaded bytes.
changeset = build_changeset(
    namespace="demo/hello",
    author="user:alice",
    timestamp_ms=1_760_659_200_000,
    message="add a greeting",
    ops=[PutOp("README.md", content="# hello\n")],
)
print(changeset.id, len(changeset.blobs))  # <64 hex> 1
# Upload with client.objects.upload(repo=repo, objects=changeset.objects()),
# then client.proposals.create(repo=repo, stream="main", head=changeset.id, title="...").
# endregion signing

# What the test checks.
signing_result = {
    "blob": blob_id("hello\n"),
    "valid": verify_provenance(record, signature, key.public_key),
    "tampered": verify_provenance({**record, "model": "other"}, signature, key.public_key),
    "changeset_id": changeset.id,
}
