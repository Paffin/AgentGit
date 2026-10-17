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
- **Backups hold GC off** while objects are copied.
- GC is safe to run while other `ag` processes work; it only contends on the store lock for individual
  deletes.

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

## Not implemented

Generational collection, packing/compression of objects and incremental mark phases. The mark phase walks
all roots' history each run, which is fine for the sizes AgentGit currently targets (see
[scaling notes](../architecture/storage.md#scaling-notes-indicative)).
