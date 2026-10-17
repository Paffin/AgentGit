# Durability, backup and restore

## What "written" means

Every write follows one rule — *objects first, durably; metadata second, in one transaction* — and each
store states its contract rather than implying one.

| Store | An acknowledged write survives | Does **not** survive |
|---|---|---|
| Object store `local-fsync` (the default) | process crash, kernel panic, power loss (if the disk honours `fsync`) | loss of the disk, host or site |
| Metadata (SQLite WAL, `synchronous=FULL`) | process crash, power loss (same caveat) | loss of the disk, host or site |

`ag init` prints the object store contract. A successful write is **not evidence of replication**. If you
need to survive losing the machine, put `.ag/` on replicated storage and/or take regular backups and
a Git mirror.

## What is where, and what each part can recover

| Part | Holds | If lost |
|---|---|---|
| `.ag/store/objects/` | all file contents and changesets | content and history *shape* are gone |
| `.ag/meta.db` | stream heads, reflog, proposals, reviews, policy versions, attestations, intents, audit, agents, leases | the objects remain, but nothing says which are the heads, who approved what or what the policy was. **Objects alone do not recover a repository.** |
| `.ag/keys/` | agent and issuer private keys | agents/issuers must be re-registered; signatures already stored stay verifiable |

Derived data (`trees`, `changeset_trees`) is disposable and rebuilt by `ag reindex`.

## Backup

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

Test: `backup_is_consistent_verified_and_independent_of_later_changes`.

## Restore

A backup is a complete repository. To restore, copy it to where you want it and check it:

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
