# Storage

## Layout

This is the default layout. A repository can instead keep its metadata in PostgreSQL and its objects in an
S3-compatible bucket; `config.json` then carries a `storage` section and `.ag/` holds only the configuration and
the keys. See [PostgreSQL and S3](../operations/postgres-and-s3.md) and [ADR-0004](../adr/0004-postgres-and-s3-backends.md).

```text
.ag/
├── config.json          {"format":1,"namespace":…,"default_stream":…[,"storage":…]} — written last by `init`
├── meta.db              SQLite (WAL); -wal and -shm files exist while open
├── store/
│   ├── objects/ab/cdef… immutable objects, 0444, fan-out by the first byte of the ID
│   ├── tmp/             write staging (empty when idle)
│   └── gc.lock          coordination between writers and garbage collection
└── keys/                private keys, 0600 (override the location with $AG_KEY_DIR)
```

## Object store

`ObjectStore` is a trait; shipped backends are the filesystem store, an S3-compatible store (below) and an
in-memory store (tests). The rules in this section hold for every backend unless stated otherwise.

- **Write**: temp file → `fsync` → `chmod 0444` → atomic `rename` → `fsync` of the directory. A reader never
  sees a partial object.
- **Read**: the envelope is parsed and the content ID **recomputed**; a mismatch is `Corrupt`, never data.
- **Idempotent and self-healing**: putting an object that exists and verifies refreshes its age and returns;
  putting one whose stored copy is damaged *replaces* it, so re-publishing repairs corruption
  (`fs_detects_corruption_and_put_heals_it`).
- **Size limit** per blob (default 256 MiB) enforced on write.
- **Durability contract**: each backend states what an acknowledged write survives
  (`ObjectStore::durability`). The filesystem store survives process crash, kernel panic and power loss
  *if the disk honours `fsync`*; it does **not** survive loss of the disk, host or site — a successful
  write is not evidence of replication. Put `.ag/store` on replicated storage or take `ag backup`s if you
  need more. See [Durability & backup](../operations/durability.md).
- **GC coordination**: every `put` holds a shared lock on `gc.lock`; `delete_if_older_than` takes it
  exclusively, re-checks the object's age and unlinks. A writer that re-puts an old object just before GC
  acts therefore always wins, closing the "collected what a writer is about to reference" race.

### S3-compatible store

`S3ObjectStore` (crate feature `s3`, on by default) speaks the S3 REST API with a hand-written AWS Signature V4
over a blocking HTTP client; there is no SDK and no async runtime on this path. Keys are
`<prefix>/objects/<2 hex>/<62 hex>`, the same fan-out and envelope as the filesystem store.

- **Write**: a conditional create (`PUT` with `If-None-Match: *`). If the object exists (HTTP 412) it is read
  back and verified; a healthy one has its age refreshed by a server-side `CopyObject` onto itself with
  `REPLACE` metadata (so `LastModified` moves and a collector keeps it); a damaged one is overwritten, which
  repairs the store. A service that ignores the conditional header rewrites identical bytes, which is
  equivalent. One request per object (no multipart), so the largest object is the repository's blob limit.
- **Read**: the envelope is parsed and the content ID **recomputed** on every `get`, exactly as for files.
- **Errors**: connection errors, timeouts and 408/409/429/5xx are retried with backoff; anything else that is not
  a success is an error. A 404 means "absent" only where absence is a valid answer, so a permission problem is
  never mistaken for a missing object.
- **Durability contract** (`ObjectStore::durability`, name `s3-compatible`): an acknowledged write survives a
  crash or loss of *this* host, and loss of a disk or node **if the service replicates or erasure-codes**
  (Amazon S3 and multi-node MinIO do). It does not survive loss of the storage of a single-node service, loss of
  a site the bucket is not replicated across, or deletion by anyone with write access (use bucket versioning).
  A 2xx is not evidence of replication — the same honesty as the filesystem store.
- **GC coordination — `ObjectLock`.** S3 gives no atomicity across requests, so writers and the collector share a
  lock: `put` (and the age refresh) hold it *shared*; `delete_if_older_than` holds it *exclusive* across its whole
  check-then-delete. Implementations: `InProcessLock` (one process), `FileLock` (processes on one host, for
  SQLite metadata) and `PostgresAdvisoryLock` (session-level advisory locks on a dedicated connection; a crashed
  process releases it automatically). `ObjectStore::block_gc` takes the shared side for backups. The race and its
  fix are reproduced in `s3_gc_cannot_delete_an_object_a_writer_just_reput`.
- Object ages are the server's `LastModified`. GC grace periods are hours, so clock differences do not matter in
  practice.

## Metadata store

Two backends behind one API: **SQLite** (the default) with `journal_mode=WAL`, `synchronous=FULL`,
`busy_timeout=30s`, `trusted_schema=OFF`, and **PostgreSQL** (`Meta::open_postgres`, one schema per repository).
All SQL lives in `crates/ag-storage/src/meta/` and is exposed as methods on a transaction handle (`Tx`),
composed by the engine inside `Meta::write` (commit on `Ok`, roll back on `Err`). Every query is written once, in the
subset of SQL both engines share, on top of a thin `Conn`/`Param`/`Row` layer (`meta/db.rs`); `?N` placeholders
become `$N` for PostgreSQL, and PostgreSQL text columns use the `"C"` collation and integers are `BIGINT` so
ordering and arithmetic match SQLite's.

### Transaction semantics per backend

The engine's correctness argument is "one writer at a time, reading a state nobody can change until it
commits" (the stream CAS and the policy evaluation happen in one such transaction). Both backends provide it:

| | SQLite | PostgreSQL |
|---|---|---|
| `Meta::write` | `BEGIN IMMEDIATE`: the database write lock, across threads and processes | `READ COMMITTED`, first statement `pg_advisory_xact_lock(key(schema))`: writers of one repository are serialized, writers of different repositories are not |
| `Meta::read` | deferred transaction: a snapshot | `REPEATABLE READ READ ONLY`: a snapshot, never blocked |
| waiting for the write lock | `busy_timeout` 30 s | `lock_timeout` 30 s |
| wait expires | `MetaError::Retriable` | `MetaError::Retriable` |
| connection lost | n/a (in-process) | `MetaError::ConnectionLost`; `during_commit: true` means the outcome is unknown |
| crashed writer | the OS releases the file lock | the server ends the session and releases the lock |
| schema version | `PRAGMA user_version` (and the `meta` table) | the `meta` table, key `schema` |
| migrations | `migrations/sqlite/NNNN_*.sql` in a `BEGIN IMMEDIATE` | `migrations/postgres/NNNN_*.sql` in a transaction holding an advisory lock |

Why `READ COMMITTED` and not a stronger level for writers: a snapshot is taken at the first statement, which is
the lock request itself — *before* the lock is granted — so a stronger level would read the state as it was
before the previous writer committed. Under `READ COMMITTED` each statement after the lock sees the latest
commit. The stream CAS is additionally a conditional `UPDATE … WHERE version = ?`, so it holds even for a
writer that did not take the lock.

PostgreSQL aborts a transaction after any failed statement, SQLite does not; a write whose closure ignored such
an error is rolled back rather than committed half done (`a_swallowed_statement_error_does_not_commit_a_half_transaction`).
The tests that pin this down: `writers_of_one_repository_are_serialized_and_see_each_others_commits`,
`many_connections_racing_the_stream_cas_never_lose_an_update`, `readers_are_never_blocked_by_a_writer_and_see_a_snapshot`,
`a_connection_killed_mid_transaction_is_an_error_and_nothing_is_applied`,
`waiting_too_long_for_the_write_lock_is_a_retriable_error`, `opening_twice_at_once_migrates_exactly_once`. The
metadata, engine, policy, lifecycle and agent suites run unchanged on PostgreSQL.

| Table | Contents | Authority |
|---|---|---|
| `changesets`, `changeset_parents`, `changeset_paths` | index of **published** changesets: generation, first-parent depth, parents, touched paths | rebuildable from objects (`ag reindex`) |
| `streams`, `reflog` | stream heads, versions; append-only head history | source of truth |
| `intents`, `intent_objects` | durable operations, idempotency keys, staged objects | source of truth |
| `outbox` | transactional events (`changeset.published`, `stream.updated`) | source of truth for the one in-process consumer, tree derivation (at-least-once); `stream.updated` is a recorded notification that nothing consumes and the next pass marks dispatched. The external event feed is the audit log, not the outbox |
| `audit` | hash-chained audit log | source of truth, tamper-evident |
| `policies` | append-only policy versions | source of truth |
| `proposals`, `reviews`, `attestations`, `pins`, `workspaces`, `queue` | collaboration state | source of truth |
| `agents`, `agent_runs`, `tasks`, `task_deps`, `leases`, `counters`, `provenance` | agent coordination; `counters` provides fencing tokens | source of truth |
| `trees`, `changeset_trees` | **derived** tree manifests | disposable cache |
| `jobs` | the asynchronous job queue (schema version 2): leased claims, retry with backoff, dedupe keys | source of truth |

There are deliberately no foreign keys, which lets garbage collection remove rows in any order. `ag verify` checks
the references that matter for safety: every object's hash, each changeset against its index row and its parents,
stream heads (published, and their cached tree against history), reflog tails, and that every pin, open-proposal
head and workspace head/base names a changeset the object store can produce (the roots garbage collection starts
from). It does not check rows that merely mention a proposal or an operation (reviews, queue entries, intents) or
cached trees other than the stream heads' (those are verified when read, or rebuilt by `ag reindex`).

## Derived data

Trees are derived (`tree(c) = apply(tree(first_parent(c)), ops(c))`) and cached sparsely:

- **keyframes**: the tree of every changeset whose first-parent depth is a multiple of 32
  (`KEYFRAME_INTERVAL`), written from the outbox by a pass that runs inline right after each publication and
  integration commits (best effort: a failed event stays pending and is retried by the next pass or `ag reindex`;
  `Repository::derive_status` reports the pending count and the last failure);
- **live heads**: the tree of each stream head, open proposal head and workspace head, written eagerly;
- everything else is rebuilt on demand by replaying fewer than 32 operation lists from the nearest cached
  ancestor (`resolve_tree`).

A cached manifest is verified on read (decode + recompute the tree ID); a bad cache entry is treated as a
miss. `ag reindex` drops and rebuilds all derived data, which doubles as a proof that it is disposable
(`derived_trees_are_disposable_and_reindex_rebuilds_them`,
`derived_tree_cache_is_sparse_but_every_tree_is_still_readable`).

## Scaling notes (indicative)

Measured on a small shared VM with a synthetic repository of **3 000 files and 1 500 commits**
(release build): import from Git 13 s; creating a workspace (checking out 3 000 files) 0.3 s; snapshot 0.2 s; propose 0.5 s;
integrate 0.8 s; `ag verify` (hash 9 000 objects, recompute a head tree through 1 500 commits) 0.4 s;
metadata DB 4 MB. These are not SLOs and they are single-process numbers on SQLite and local files (no network, no PostgreSQL or S3
latency, see [ADR-0006](../adr/0006-flat-trees-measured.md)); load numbers for the server are not published. They show where the design stands.

Known limits:

- trees are **flat manifests** (O(files) per derived tree and per merge); hierarchical Merkle directories
  are on the [roadmap](../../ROADMAP.md);
- blobs are read fully into memory (bounded by the blob limit), and snapshots hash every file of the
  working directory;
- writers of one repository are serialized, on every backend: throughput per repository is bounded by the round
  trips of a short transaction. SQLite is a local file (one host); PostgreSQL and S3 lift that limit for the number
  of processes and hosts, not for concurrent writers of the *same* repository.
