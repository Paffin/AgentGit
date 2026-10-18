# Object formats

Everything that is hashed or signed has exactly one valid byte representation, produced and parsed by
strict code. Protobuf (or any transport format) never participates in hashing.

The formats are checked against an **independent implementation**: `tests/conformance/gen_vectors.py`
(plain Python, no shared code) writes `tests/conformance/vectors.json`, and Rust tests verify byte-for-byte
equality for: blob IDs and `ChangesetV1` payloads and IDs (`ag-changeset`), tree manifests and tree IDs
(`ag-tree`), attestation signing bytes (`ag-policy`) and the audit hash chain (`ag-storage`). Ed25519
signatures themselves and the provenance JSON are covered by Rust tests only. See
[`tests/conformance/README.md`](../../tests/conformance/README.md) for how to add a vector.

## Content IDs

```text
id = SHA-256( "agentgit.object\0" ‖ kind:u8 ‖ version:u16be ‖ length:u64be ‖ payload )
```

| kind | tag | stored? |
|---|---:|---|
| blob | 1 | yes |
| changeset | 2 | yes |
| tree | 3 | no — derived; the ID names an exact state |

The domain prefix, kind and version are part of the preimage, so a blob can never collide with a
changeset that happens to have the same bytes, and a format bump changes every ID.

A stored object (the *envelope*) is `"AGOB" ‖ kind:u8 ‖ version:u16be ‖ payload`. The ID is not stored;
it is the file name and is **recomputed on every read** (`ObjectStore::get` fails with `Corrupt` on mismatch).

## Canonical encoding primitives (`ag_object::canon`)

- fixed-width integers are big-endian (`u16`, `i64`);
- `varint` is unsigned LEB128 and **must be minimal** — redundant continuation bytes are rejected;
- byte strings and UTF-8 strings are `varint length ‖ bytes`, with a caller-supplied maximum so a hostile
  length prefix cannot cause a large allocation;
- a decoder rejects trailing bytes, so `encode(decode(b)) == b` for every accepted `b`
  (property-tested: `accepted_varints_reencode_identically`, `roundtrip_random_changesets`).

## `ChangesetV1` (payload of kind 2)

```text
format_version : u16be            = 1
namespace      : str              repository namespace, part of the hash
parents        : varint n, n × 32 bytes
author         : str              identity, e.g. "agent:bot#run_ab12"
timestamp_ms   : i64be            author time, Unix milliseconds
message        : str
ops            : varint n, n × op
op             : 0x01 path:str file_type:u8 executable:u8 content:32 bytes     (put)
               | 0x02 path:str                                                 (delete)
```

Rules enforced on build **and** on decode:

- operations are sorted **strictly ascending** by path (bytewise) — one path at most once;
- no more than `max_parents` (16) parents, none duplicated; `max_ops` (100 000) operations;
  message ≤ 64 KiB;
- `file_type` is 0 (regular) or 1 (symlink); `executable` is 0/1 and never set on a symlink;
- every path, identity and namespace is re-validated (below), so a hand-crafted payload containing
  `../etc/passwd` is rejected at decode (`decode_rejects_traversal_paths`).

Semantics: `ops` is the delta against the **first parent** (empty state for a root). A put replaces/creates;
a delete must hit an existing entry; a put must actually change something (the delta is *minimal*, which
keeps equal states reachable by equal operation lists). No path may be both a file and a directory.
These are checked when applying (`Tree::apply`) and during publication verification.

## Tree manifest v1 (kind 3, derived)

```text
varint n, n × ( path:str file_type:u8 executable:u8 content:32 bytes )     sorted strictly ascending
```

`TreeId = SHA-256(domain ‖ 3 ‖ 1 ‖ len ‖ manifest)`. Equal repository states have equal tree IDs
regardless of the history that produced them. Decoding re-validates ordering and the file/directory rule.

## Value types (`ag-types`)

| Type | Rules |
|---|---|
| `RepoPath` | UTF-8, relative, `/`-separated, ≤ 4096 bytes, components ≤ 255; no empty, `.` or `..` components; no NUL/control characters or `\`; no `.git` or `.ag` component under any spelling a file system folds into it (ASCII case-insensitive; NTFS trailing dots and spaces, `:` alternate-stream suffixes and `git~N` short names; HFS+ ignorable code points: `crates/ag-types/src/portable.rs`) |
| `Identity` | `user:`/`agent:`/`service:`/`system:` + `[A-Za-z0-9._@#+-]{1,128}`, not starting with `#` (the part before the first `#` is the principal and must itself be an identity; [ADR-0009](../adr/0009-identity-names-and-principals.md)) |
| `Namespace` | `[a-z0-9][a-z0-9._-]*` segments separated by `/`, ≤ 128 |
| `StreamName` | same alphabet as namespace, ≤ 64 |
| `OperationId` | `[A-Za-z0-9_-]{8,64}` |

Resource limits (`Limits::default()`): blob 256 MiB, 100 000 ops/changeset, 16 parents, 64 KiB message,
DAG depth 10 000 000, 10 000 changesets per publication, 2 000 000 tree entries.

## Signatures

All signatures are Ed25519 (strict verification) over a **domain-separated** message:

```text
signed = "agentgit.sig.v1\0" ‖ len(domain):u8 ‖ domain ‖ message
```

| Domain | Message |
|---|---|
| `attestation.v1` | `str id ‖ tree:32 ‖ str check ‖ result:u8 (1 pass, 2 fail) ‖ varint policy_version ‖ str environment_digest ‖ str issuer ‖ i64 issued_at_ms` |
| `provenance.v1` | compact JSON of the provenance record with keys sorted (schema `agentgit.provenance.v1`) |

Because the domain is signed, a signature made for one purpose cannot be replayed for another, and
because an attestation covers the tree ID, policy version and environment digest, it cannot be moved to a
different state.

## Audit chain

```text
hash_i = SHA-256( str "agentgit.audit.v1" ‖ str hash_{i-1} ‖ i64 ts ‖ str actor ‖ str action ‖ str target ‖ str details )
```

with `hash_0 = "0"×64`. Each entry is appended in the same transaction as the state change it records.
`ag audit` recomputes the chain and names the first broken entry. This makes edits and deletions
*detectable*; it does not stop someone with write access to the database from rewriting the whole chain,
which is why `ag audit` prints the head hash — record it somewhere you trust.
