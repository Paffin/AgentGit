# Durability, backup and restore

## What "written" means

Every write follows one rule — *objects first, durably; metadata second, in one transaction* — and each
store states its contract rather than implying one.

| Store | An acknowledged write survives | Does **not** survive |
|---|---|---|
| Object store `local-fsync` (the default) | process crash, kernel panic, power loss (if the disk honours `fsync`) | loss of the disk, host or site |
| Metadata (SQLite WAL, `synchronous=FULL`) | process crash, power loss (same caveat) | loss of the disk, host or site |
| Object store `s3-compatible` | crash or loss of the client host; loss of a disk or node **if the service replicates or erasure-codes** (Amazon S3, multi-node MinIO) | loss of a single-node service's storage; loss of a site the bucket is not replicated across; deletion by anyone with write access (turn on bucket versioning) |
| Metadata (PostgreSQL) | whatever the server is configured to survive: `COMMIT` returns after the WAL is flushed (`synchronous_commit = on`, the default); with replicas or PITR, loss of the primary | a server with `fsync = off` or `synchronous_commit = off`, or a single unreplicated disk, loses acknowledged commits like any database |

`ag init` prints the object store contract. A successful write is **not evidence of replication**. If you
need to survive losing the machine, put `.ag/` on replicated storage and/or take regular backups and
a Git mirror — or keep the repository in PostgreSQL and S3 on infrastructure with the durability you need
([setup](postgres-and-s3.md)). Which of those guarantees you actually have is a property of the servers you run,
not of `ag`; this page only tells you where to look.

## What is where, and what each part can recover

| Part | Holds | If lost |
|---|---|---|
| `.ag/store/objects/` (or the S3 prefix) | all file contents and changesets | content and history *shape* are gone |
| `.ag/meta.db` (or the PostgreSQL schema) | stream heads, reflog, proposals, reviews, policy versions, attestations, intents, audit, agents, leases, jobs | the objects remain, but nothing says which are the heads, who approved what or what the policy was. **Objects alone do not recover a repository.** |
| `.ag/keys/` | agent and issuer private keys | agents/issuers must be re-registered; signatures already stored stay verifiable |

Derived data (`trees`, `changeset_trees`) is disposable and rebuilt by `ag reindex`.

## Backup

What to do depends on where the repository lives.

| metadata | objects | how to back it up |
|---|---|---|
| SQLite | files | `ag backup` (below) |
| SQLite | S3 | `ag backup`: the objects are downloaded, verified one by one, into a plain local repository |
| PostgreSQL | any | **not `ag backup`** — it refuses, says so, and writes nothing. Use [PostgreSQL's tools](#postgresql-and-s3) |

### SQLite and files: `ag backup`

```bash
ag backup /backups/project-2026-10-17            # new or empty directory
ag backup /backups/project-with-keys --include-keys   # keys are NOT copied unless you ask
```

How it stays consistent while others write:

1. the database is copied first with `VACUUM INTO` (a transactionally consistent snapshot);
2. then the objects are copied. Objects are immutable and always written *before* the metadata row that
   references them, so everything the snapshot references already exists; the copy may include a few newer,
   unreferenced objects (garbage, collected later);
3. garbage collection is held off for the duration (it needs the store's exclusive lock);
4. the result is opened and fully verified (`ag verify` semantics). The command exits non-zero if the backup
   is not healthy.

Do **not** back up by copying `.ag/` with `cp`/`rsync` while `ag` is running: the SQLite file and its WAL
must be read as one snapshot. (Copying while *nothing* is running is fine.)

Test: `backup_is_consistent_verified_and_independent_of_later_changes`; with S3 objects:
`backup_downloads_s3_objects_into_a_self_contained_local_repository`.

### PostgreSQL and S3

A database cannot be copied faithfully by `ag`, so use the database's own tooling, and treat the two stores as
**two backups that must agree on reachability**: every object the metadata snapshot references must exist in the
object backup. The same ordering argument as for `ag backup` applies — objects are written before the metadata row
that makes them reachable — so:

1. Back up **PostgreSQL first**, then the bucket. For one repository: `pg_dump --schema=<schema> --format=custom`.
   For everything on the server, or to restore to a point in time: a base backup plus WAL archiving (PITR).
2. Back up the **objects second**: a replicated or versioned bucket, or a copy (`aws s3 sync`, `mc mirror`) of the
   prefix made *after* the dump started. A copy may contain objects the dump does not reference; they are garbage
   and `ag gc` collects them later. Do not run `ag gc` while the copy runs: `ag backup` holds collection off
   itself, a plain bucket copy does not (collection only deletes unreachable objects older than the grace period,
   24 h by default, so a short copy is safe unless you shortened `--grace`).
3. **Restore, then reconcile**: restore the database, restore or keep the bucket, run `ag verify`. It reports a
   metadata row whose object is missing (`bad_blob`, `missing_object`) and any damaged object. Do not assume two
   independent backups agree — the check is the point. The audit **head hash** printed by `ag verify` should match
   the one you recorded.

The `pg_dump` / restore round trip of a repository whose objects stay in the bucket was exercised by hand with the
real CLI (init, integrate, `pg_dump`, `DROP SCHEMA`, `pg_restore`, then `ag verify` healthy with the same audit
head); backing up *and* restoring the bucket is the bucket provider's feature and was not exercised.

A restored repository needs the same `storage` section (the schema name and the bucket prefix are part of it) and
the same environment variables.

## Restore

A backup made by `ag backup` is a complete repository. To restore, copy it to where you want it and check it:

```bash
cp -a /backups/project-2026-10-17 /srv/project
cd /srv/project && ag verify
```

Keys are only present if you backed them up with `--include-keys`; otherwise restore them from your own
secret storage (or register new agents). The audit chain and reflog carry over unchanged.

## Verify regularly

```bash
ag verify
```

hashes every object, cross-checks every published changeset against the index, recomputes stream-head trees
from history, checks that each reflog ends at its stream's head, and verifies the audit chain. It prints the
audit **head hash**: record it somewhere you trust; if the chain is ever rewritten wholesale, the head hash
will no longer match.

## A Git mirror as a second line

`ag git export main /srv/mirror.git --branch main` gives you a second, independent copy of the history in a
format every tool reads. It does not contain proposals, reviews, policy, attestations or provenance — it is
a content safety net, not a replacement for `ag backup`.
