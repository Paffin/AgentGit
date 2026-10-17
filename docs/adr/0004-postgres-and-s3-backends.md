# ADR-0004: PostgreSQL metadata and S3-compatible objects, behind the existing seams

- Status: accepted
- Date: 2026-10-17
- Amends: [ADR-0002](0002-embedded-engine-first.md) (migration path, steps 2 and 3) and realizes the storage
  choices of [ADR-0001](0001-final-architecture.md) §3, §5 and §10

## Context

ADR-0002 shipped the engine on SQLite and a filesystem store and promised PostgreSQL and S3 later, "extracting the
trait from the concrete surface". A network server needs both: several processes and hosts on one repository, no
local disk as the source of truth. The guarantees must not weaken — in particular the stream compare-and-swap with
policy evaluated on facts read inside the committing transaction (it relies on a *stable* view while the
transaction runs), and garbage collection that cannot delete an object a writer has just re-stored.

## Decisions

1. **One SQL, two dialects — not a domain trait.** ADR-0002 expected a trait extracted from the surface. The
   surface is over a hundred methods on `Tx`; a trait over them would be written twice for no gain. Instead there is a
   thin seam *below* them (`meta/db.rs`): `Conn::execute/query` with typed `Param`/`Row`, and every query is
   written once in the common subset of SQLite and PostgreSQL (`?N` placeholders rewritten to `$N`,
   `ON CONFLICT`, `RETURNING id`, `BIGINT` only, `lower(x) LIKE lower(?)` where SQLite's case-insensitive
   `LIKE` is relied upon). Differences live in the migrations, in how a transaction is opened, and in one
   `FOR UPDATE SKIP LOCKED`. The domain API (`Meta`, `Tx`) did not change.
2. **Writers are serialized by an advisory lock, under `READ COMMITTED`.** SQLite's `BEGIN IMMEDIATE` gives "the one
   writer, reading the latest state". PostgreSQL's equivalent is `pg_advisory_xact_lock(key(schema))` as the first
   statement of a write transaction. The isolation level matters: `REPEATABLE READ`/`SERIALIZABLE` would pin the
   snapshot at that first statement — *before* the lock is granted — and read stale data; at `READ COMMITTED` every
   statement after the lock sees the previous writer's commit. Reads use `REPEATABLE READ READ ONLY` (a snapshot, no
   locks). Rejected: serializable transactions with retry (the engine's single-writer reasoning would turn into
   abort storms), and a row lock on the stream row (policy facts also live in other tables). The stream CAS is
   still a conditional `UPDATE`, so it stays correct even if some writer skipped the lock. A lock wait is bounded
   (`lock_timeout`, 30 s, like SQLite's `busy_timeout`) and expires as a *retriable* error.
3. **Failure is never silent.** A lost connection is `MetaError::ConnectionLost`, flagged *unknown outcome* when it
   strikes during `COMMIT` (the engine's answer is the existing "ask by operation id"). A statement error swallowed
   by the closure turns into a rollback, because PostgreSQL would otherwise commit nothing while the caller sees
   success. A connection that was in a transaction when its thread unwound is discarded, never reused.
4. **One schema per repository.** The schema is the unit of isolation and of the lock key; `search_path` is set to
   that schema alone. Migrations run in one transaction under another advisory lock, so any number of starting
   processes is safe, and a newer schema is refused. `Repository::open` never creates a schema (a dropped schema or a
   wrong URL must not silently become an empty repository); only `init` does.
5. **Byte-wise text, 64-bit integers.** PostgreSQL text columns are `COLLATE "C"` and integers `BIGINT`, so ordering,
   range scans (`id >= p AND id < p || 'g'`) and arithmetic equal SQLite's. Sequences (`BIGSERIAL`) can leave gaps
   after a rollback; order and uniqueness hold because writers are serialized.
6. **Numbered migrations in `migrations/{sqlite,postgres}/`**, compiled in with `include_str!`; a test fails if a
   file is not listed or the backends disagree. Schema 2 adds the `jobs` table ("PostgreSQL-backed queue",
   ADR-0001 §10) for both backends.
7. **Object store: conditional create, server-side refresh, hash on every read.** A hand-written SigV4 client over
   `ureq` + rustls (no SDK, no async runtime in the object path) creates objects with `If-None-Match: *`; for an
   existing object it verifies, and refreshes the age with `CopyObject` onto itself with `REPLACE` metadata. A
   service that ignores the conditional header rewrites identical bytes, which is equivalent. Retries only for
   transient failures; every other non-success is an error.
8. **`ObjectLock` instead of pretending S3 is atomic.** `put`/refresh hold it shared, `delete_if_older_than` holds
   it exclusive across check *and* delete. `InProcessLock` (tests), `FileLock` (one host, SQLite metadata) and
   `PostgresAdvisoryLock` (session-level advisory locks on a dedicated connection: a crashed holder is released by
   the server; no leases to expire). Rejected: relying on `LastModified` alone (a writer between check and delete
   loses its object — reproduced in `s3_gc_cannot_delete_an_object_a_writer_just_reput`), conditional deletes (not
   available on general-purpose buckets), versioned buckets as a safety net (an assumption about the operator).
9. **Secrets are never in the repository.** `storage` in `.ag/config.json` names environment variables; a literal URL
   with a password is refused. Credentials are static environment credentials; no credential-provider chain.
10. **A pre-storage `ag` fails loudly.** `format` stays 1 and the section is additive, which means an older binary
    ignores it. `init` therefore occupies `.ag/meta.db` (PostgreSQL) / `.ag/store` (S3) with a placeholder so the old
    binary cannot create an empty local repository next to the real one.
11. **`ag backup` does not half-work.** With PostgreSQL metadata it refuses before writing anything and points at
    `pg_dump`/PITR plus `ag verify`; with S3 objects it downloads and verifies them into a plain local repository.

## Consequences

- ✅ The same engine and the same test suites run on SQLite+files, PostgreSQL, S3 and combinations
  (`AG_TEST_POSTGRES_URL`, `AG_TEST_S3_ENDPOINT`; `AG_REQUIRE_BACKENDS=1` turns a missing server into a failure).
- ✅ Several hosts can work on one repository; writers of different repositories never contend.
- ⚠️ Writes to one repository stay serialized (as with SQLite); the unit of scaling is the repository.
- ⚠️ SQLite metadata with S3 objects is single-host; cross-host needs PostgreSQL metadata (the lock).
- ⚠️ No tool yet moves an existing repository between backends, and PostgreSQL behind a transaction-pooling proxy is
  unsupported (session state is used).
- ⚠️ Verified against PostgreSQL 16 and moto; not yet against Amazon S3 or MinIO themselves (see
  [the operations guide](../operations/postgres-and-s3.md#honest-limits)).
- ⚠️ The `migrations/` directory sits outside the crate, so `cargo package` for `ag-storage` needs the files copied
  or symlinked in before publishing.
