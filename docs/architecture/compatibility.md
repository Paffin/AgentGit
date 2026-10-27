# Compatibility

What a newer AgentGit promises about data an older one wrote, how that is tested, and what to do when a persisted
format has to change. The short version: **a build opens what an older build wrote, or refuses it with a message;
it never reads it differently.**

## What is persisted, and how it is versioned

| State | Where | Version | A newer build ... | An older build ... |
|---|---|---|---|---|
| Objects (blob, `ChangesetV1`, tree manifest) | object store (files or S3) | a kind byte and a `u16` version in the envelope, and the version is part of the hash preimage ([Object format](object-format.md)) | reads every version it knows | refuses an unknown kind or version |
| Repository metadata (streams, reflog, proposals, policies, attestations, agents, jobs, audit) | `meta.db` (SQLite) or the PostgreSQL schema | `PRAGMA user_version` / `meta` row `schema`, currently **3** (`migrations/sqlite`, `migrations/postgres`) | migrates forward on open, in one transaction | refuses with `UnsupportedSchema` |
| Server control plane (credentials, role bindings, repository registry with per-repository configuration, idempotency records) | `<data_dir>/control.db` or the PostgreSQL schema of `[control]` | `PRAGMA user_version`, currently **3** (`migrations/control`) | migrates forward on open; replicas that start together migrate once, under an advisory lock | refuses with `the control database has schema version N, newer than this server supports` |
| Audit chain | `audit` table | the hash of each entry covers the previous one | verifies old entries | - |
| Policy documents, attestation signing bytes, provenance records | `policies`, `attestations`, `provenance` | the signed bytes are specified in the object-format page; a policy version change invalidates earlier attestations by design | verifies old signatures | - |
| Wire protocol | AGWP v1: `proto/agwp/v1/agwp.proto`, OpenAPI at `docs/protocol/openapi.json` | the URL prefix `/v1`; additive changes only (new fields and operations; an empty value means "as before") | serves old clients | an old client ignores fields it does not know |
| CLI `--json` output | stdout | golden shapes in `crates/ag-cli/tests/json_shapes.rs` | keeps the documented fields | - |

Rules that follow from the table:

* **A change to the bytes that are hashed or signed is a new format version and needs an ADR** (`docs/adr/`). It is
  never a quiet edit: an old repository would stop verifying.
* **Migrations are forward-only and append-only.** A new `NNNN_name.sql` per backend; an old one is never edited. A
  unit test per metadata store fails when a file in `migrations/` is not in the migration list.
* **A schema newer than the build is refused, never "best-effort read".** The error names both versions.
* **Additive wire changes only inside `v1`.** A new field has a value that means "as before" (an empty `run_id` is a
  credential not bound to a run; the old fixtures report no such field at all and still pass).

## Fixtures: bytes an old build really wrote

Tests that write and read with the same build prove nothing about compatibility, so the repository keeps *files
written by old builds* in `tests/compat/fixtures/` and runs every one of them through the current code.

| Fixture | Written by | Holds | Test |
|---|---|---|---|
| `repo-0.1.0.tar.gz` (+ `.expected.json`) | the `ag` of the first alpha (`tests/compat/generate.sh`), meta schema 2 (migrated to 3 when opened: [ADR-0011](../adr/0011-proposal-paging-indexes-and-background-leases.md)) | a repository with clean and conflict-resolved merges, a rename-aware merge, two policy versions, signed attestations, agents, runs, a task, provenance, a lease, a pin, a second stream and the audit chain | `crates/ag-integrate/tests/compat.rs` (8 tests) |
| `repo-0.2.0-rc.2.tar.gz` (+ `.expected.json`) | the `ag` built with the proposal-paging indexes (`tests/compat/generate.sh 0.2.0-rc.2`), meta schema **3** | the same content as `repo-0.1.0` | same |
| `control-schema2.tar.gz` (+ `.expected.json`) | `ag-server` built from commit `e5a8146`, the first with a versioned control database (`tests/compat/generate_server.sh`) | two repositories, four credentials (one revoked), four role bindings, an idempotency record, control schema **2** | `crates/ag-server/tests/compat_control.rs` (6 tests) |
| `control-schema3.tar.gz` (+ `.expected.json`) | `ag-server` built from commit `b69669d` | the same, plus an agent, a run, a delegator and a credential bound to that run (ADR-0010), control schema **3** | same |

What the tests check:

* **Repository** - `fsck` is clean; stream heads, changeset ids, tree ids and the order of history are exactly the
  recorded ones, which were cross-checked against the independent Python encoder (`tests/conformance/gen_vectors.py`)
  when the fixture was made; **every merge the old build made is reproduced bit for bit by today's merge** (a change
  of merge semantics shows up here); blame follows the rename as before; policies, attestations and provenance still
  verify; agents, tasks, leases and pins are readable; new work can be published on top.
* **Control database** - every recorded token still authenticates as the same identity with the same roles and
  scope (a field added since must be empty); a revoked token stays revoked; garbage does not authenticate;
  repositories and role bindings are the recorded ones; an out-of-scope repository stays out of reach; the old
  idempotency key still replays the same request and still refuses a different one; a run-bound credential acts as
  its run and dies with it; the old data directory accepts new work and its audit chain verifies; **the database is
  migrated to this build's schema when the server opens it** (`schema2` is upgraded to 3 with real data); and a
  database a *newer* build wrote (`user_version` raised by one) is refused with the message above and left untouched.
  The last test found a defect when it was written: a newer SQLite control database was reported as `internal
  error; see the server log`, only the PostgreSQL path had the explanatory message. Both paths say it now.
* **Independent implementations** - `tests/conformance/vectors.json` is produced by a from-scratch Python encoder
  that shares no code with Rust and is read by the Rust crates (`ag-changeset`, `ag-tree`, `ag-policy`,
  `ag-storage` conformance tests) and by both SDKs (`sdk/typescript/test/harness/vectors.ts`,
  `sdk/python/tests/test_conformance.py`), so the *specification* and not only the code is held to the bytes.

## Adding a fixture

Fixtures are **append-only**. Never regenerate or edit one: its whole value is that an old build wrote it.

1. A persisted format or schema is about to change (a new `migrations/.../NNNN_*.sql`, a new hashed field, ...).
2. Build the generator's subject **before** the change if you want a fixture that proves the *upgrade* (export the
   previous commit with `git archive`, build it in a scratch directory, copy the binary out), or after it if you want
   one that pins the *new layout*. Repository: `AG=/path/to/ag tests/compat/generate.sh <label>`. Server:
   `FIXTURE_BUILT_FROM=<commit> AG_SERVER=/path/to/ag-server tests/compat/generate_server.sh <label>`
   (`FIXTURE_RUN_CREDENTIALS=1` for a server that has the delegator role and run-bound credentials).
3. Commit the `.tar.gz` and `.expected.json`. The tests pick every `control-*.tar.gz` and `repo-*.tar.gz` up by
   name; nothing else changes. The repository generator refuses to write a fixture when the Rust and Python
   encoders disagree about an id.
4. If an old fixture now fails, that is the finding: either the change breaks old data (fix the change, or write the
   migration) or the format change is deliberate (ADR, version bump) and the old fixture keeps passing through the
   old-version code path.

## Not covered

* **PostgreSQL and S3 fixtures.** The fixtures are SQLite + file-system. The PostgreSQL migrations are exercised
  against a real PostgreSQL by `scripts/pg-test.sh` (including the refusal of a newer schema), but no
  PostgreSQL-resident *old data* is kept.
* **One released generation.** No release has been tagged yet; `0.1.0` is the label of the first alpha build, and
  the repository fixture was made at meta schema 2. The upgrade from meta schema 1 is covered by a unit test that
  rewinds a database (`a_version_1_database_migrates_forward_and_keeps_its_data`), not by a file an old build wrote.
* **Windows.** The fixtures are unpacked with `tar` and contain a symlink; the compatibility tests are Unix-only.
* **The wire.** Old *clients* against the new server are covered by the OpenAPI golden and the SDK generators staying in step with the protocol (their
  `--check` gates), not by recorded traffic from an old client build.
