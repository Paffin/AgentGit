# Git bridge

Git is an **adapter**, not the engine. The bridge uses the `git` command line (plumbing only), so it needs
`git` on `PATH` and never links Git's code. A walkthrough whose commands are run by the tests, including a check that an
import followed by an export gives back the same trees, is [Migrating a Git
repository](../tutorials/migrating-from-git.md).

## Import a Git repository

```bash
ag init --namespace me/project
ag git import /path/to/git/repo --rev HEAD --stream imported
ag log imported
ag verify
```

Every commit becomes a changeset with the **same parents in the same order**; operations are the diff
against the first parent, which is exactly AgentGit's changeset semantics, so merges carry over without
interpretation. Executable bits and symlinks are preserved. The author becomes `user:<email>`
(sanitized), the commit time is kept, and the original SHA is appended to the message:

```text
Imported-From: git 5f2c0e…
```

The target stream must not have history yet (`import` never rewrites an existing stream). The whole
imported history is published in one verified operation.

**Not carried over:** submodules (gitlinks), paths AgentGit refuses to record (non-UTF-8, control characters,
backslashes, and any `.git`/`.ag` component — a repository can contain such trees via plumbing, and checking them
out would let it plant hooks), tags, notes, signatures, committer identity, timezone offsets, and every ref
other than the one `--rev` names. **What is reported:** each skipped path, the number of submodule entries, and
the number of commits whose text was not valid UTF-8 (`lossy_commits` in `--json`; git converts a commit that
names its encoding, anything else that is not UTF-8 is repaired with U+FFFD). The rest is dropped without a
message: a migration that needs signatures, tags or the committer must keep Git as the record of them.

**What git prints cannot be changed from outside.** The bridge asks `git log` for NUL-separated fields, so a
commit message may hold any character; it passes `--no-show-signature` and `--encoding=UTF-8`, so
`log.showSignature` and `i18n.logOutputEncoding` in your configuration change nothing; and it removes `GIT_DIR`,
`GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_COMMON_DIR`, `GIT_NAMESPACE`,
`GIT_ALTERNATE_OBJECT_DIRECTORIES` and `GIT_PREFIX` from the environment of every git process, so `ag git
import|export` run from a git hook works on the repository it was given and not on the hook's. If `git cat-file`
fails, the error is git's own message.

## Export a stream to Git

```bash
ag git export main /path/to/new-or-existing/git/repo --branch main
```

Uses `git fast-import`. The branch name is checked by `git check-ref-format` before anything is created, so a
refused name leaves no repository behind. The destination is `git init`-ed if missing. An identity such as `user:alice`
becomes the author `user:alice <user.alice@agentgit.invalid>`; messages, timestamps, file modes and
symlinks are preserved.

## Round-trip guarantee

For histories without skipped paths, **import → export reproduces the same Git tree SHA for every
commit** (and the same parent structure). This is tested on a history containing modified/deleted files,
executable files, symlinks, empty and binary files, a non-ASCII path, a branch and `--no-ff` merge,
directory↔file replacement, a mode-only change and a file→symlink type change
(`history_survives_a_round_trip_with_identical_git_trees`). Commit SHAs differ, because identities and
timestamps are mapped; the `Imported-From:` trailer lets you pair commits.

## Using both

A practical migration path: keep Git as the system of record, run `ag git import` periodically into a
scratch stream to let agents work with AgentGit's policy and leases, and `ag git export` the result back.
The bridge is one-shot per direction, not a live mirror.
