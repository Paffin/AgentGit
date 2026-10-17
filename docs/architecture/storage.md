# Storage

## Layout

```text
.ag/
├── config.json          {"format":1,"namespace":…,"default_stream":…} — written last by `init`
├── meta.db              SQLite (WAL); -wal and -shm files exist while open
├── store/
│   ├── objects/ab/cdef… immutable objects, 0444, fan-out by the first byte of the ID
│   ├── tmp/             write staging (empty when idle)
│   └── gc.lock          coordination between writers and garbage collection
└── keys/                private keys, 0600 (override the location with $AG_KEY_DIR)
```

## Object store

`ObjectStore` is a trait; shipped backends are the filesystem store and an in-memory store (tests).

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

## Metadata store

SQLite with `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=30s`, `trusted_schema=OFF`. The schema
version is in `PRAGMA user_version`; a database written by a *newer* version is refused. All SQL lives in
`crates/ag-storage/src/meta/` and is exposed as methods on a transaction handle (`Tx`), composed by the
engine inside `Meta::write` (`BEGIN IMMEDIATE`, commit on `Ok`, roll back on `Err`).

| Table | Contents | Authority |
|---|---|---|
| `changesets`, `changeset_parents`, `changeset_paths` | index of **published** changesets: generation, first-parent depth, parents, touched paths | rebuildable from objects (`ag reindex`) |
| `streams`, `reflog` | stream heads, versions; append-only head history | source of truth |
| `intents`, `intent_objects` | durable operations, idempotency keys, staged objects | source of truth |
| `outbox` | transactional events (`changeset.published`, `stream.updated`) | source of truth (at-least-once delivery) |
| `audit` | hash-chained audit log | source of truth, tamper-evident |
| `policies` | append-only policy versions | source of truth |
| `proposals`, `reviews`, `attestations`, `pins`, `workspaces`, `queue` | collaboration state | source of truth |
| `agents`, `agent_runs`, `tasks`, `task_deps`, `leases`, `counters`, `provenance` | agent coordination; `counters` provides fencing tokens | source of truth |
| `trees`, `changeset_trees` | **derived** tree manifests | disposable cache |

There are deliberately no foreign keys: referential consistency is checked by `ag verify`, which lets
garbage collection remove rows in any order.

## Derived data

Trees are derived (`tree(c) = apply(tree(first_parent(c)), ops(c))`) and cached sparsely:

- **keyframes**: the tree of every changeset whose first-parent depth is a multiple of 32
  (`KEYFRAME_INTERVAL`), written asynchronously from the outbox;
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
metadata DB 4 MB. These are not SLOs — benchmarks and load targets are an explicit non-goal until the
server exists (ADR-0001 §1) — but they show where the design stands.

Known limits:

- trees are **flat manifests** (O(files) per derived tree and per merge); hierarchical Merkle directories
  are on the [roadmap](../../ROADMAP.md);
- blobs are read fully into memory (bounded by the blob limit), and snapshots hash every file of the
  working directory;
- the engine is single-node: SQLite serializes writers.
