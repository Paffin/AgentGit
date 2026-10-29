# Garbage collection and retention

```bash
ag gc --dry-run                       # report only
ag gc                                 # delete unreachable objects older than the grace period
ag gc --grace 72h --reflog-retention 180d
```

## What is kept

An object is deleted only if **all** of these hold:

1. it is not reachable from a root;
2. it is not staged by an in-flight (non-terminal, or recently finished) publication;
3. it is older than `--grace` (default 24 h);
4. at the moment of deletion, it is *still* older than the grace cutoff (see "races").

**Roots** are: every stream head; every pin (`ag pin <name> <rev>`); every **open** proposal head; every
workspace head and base; and every head that appeared in a stream's **reflog** within
`--reflog-retention` (default 90 days — so recently abandoned history stays recoverable). Reachability
follows parents and the blobs each changeset references.

## Safety rules

- **Fail closed.** If any object reachable from a root is missing, unreadable or fails verification, GC
  aborts *before deleting anything* and tells you which one
  (`gc_fails_closed_when_a_reachable_object_is_damaged`). Run `ag verify`.
- **No lost races.** The classic hazard is deleting an old unreachable blob that a writer is just about to
  reference. Writers re-`put` content before referencing it, which refreshes the object's age, and the
  deletion is performed by `delete_if_older_than`, which takes the store's exclusive lock and re-checks the
  age. A writer that finishes first wins; one that starts after simply rewrites the object
  (`rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc`).
- **With S3 the lock is an `ObjectLock`.** A bucket has no `flock`, and two HTTP requests (check the age, delete)
  are not atomic. Writers hold the repository's object lock *shared* around `put` and the age refresh; the
  collector holds it *exclusive* across one object's check-then-delete. Without it a writer's `put` could return
  between the collector's check and its delete and the object would be gone
  (`s3_gc_cannot_delete_an_object_a_writer_just_reput` shows the loss without the lock and its absence with it;
  `rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc_on_s3` runs the scenario through the engine).
  The lock must span **every process that writes to or collects from the prefix**: with PostgreSQL metadata the
  default is PostgreSQL advisory locks (any host; a crashed process releases it by itself), with SQLite metadata it
  is a file lock (this host only). Do not run several hosts against one prefix with `file` or `in_process` locks.
  See [PostgreSQL and S3](postgres-and-s3.md#the-object-lock).
- **Backups hold GC off** while objects are copied.
- GC holds the repository metadata writer transaction from its root snapshot through deletion and index cleanup.
  Other metadata writers wait until that pass finishes; reads on independent database connections can continue.
  Creating a pin, proposal or workspace root validates newly retained history in its committing transaction.
  A root created first is retained; a request for already swept history is rejected instead of acknowledged.
  Schedule large passes off-peak, especially with remote object storage.
- On S3, "older than the grace period" is the server's `LastModified` (one-second resolution on Amazon S3), and a
  re-`put` refreshes it with a server-side copy. Grace periods are hours, so clock differences between machines and
  the service do not matter in practice. Never put a lifecycle *expiration* rule on the prefix: deletion belongs
  to `ag gc`, which knows what is reachable.

## After deletion

Index rows of deleted changesets are removed (children before parents), orphaned derived trees are pruned,
and a `gc.run` audit entry is written. Closed and integrated proposals keep their metadata; their heads may
no longer exist once past the grace period and retention window.

## Retention knobs

| To keep… | Do |
|---|---|
| a release forever | `ag pin v1.0 <rev>` |
| abandoned experiments for a while | rely on `--reflog-retention` (heads that were ever a stream head) or pin them |
| unproposed workspace snapshots | they are roots as long as the workspace exists (`ag workspace list`); removing a workspace releases them |
| everything, for now | don't run `ag gc` |

## What it costs on S3

`ag gc` lists the prefix once (paged), reads every object reachable from a root to verify it (the fail-closed
rule), and for each candidate issues one `HEAD` and, if it is still due, one `DELETE`. A repository with many
objects therefore makes many requests; run GC off-peak and keep the `ag` process near the bucket.

## Not implemented

Generational collection, packing/compression of objects and incremental mark phases. The mark phase walks
all roots' history each run, which is fine for the sizes AgentGit currently targets (see
[scaling notes](../architecture/storage.md#scaling-notes-indicative)).
