# Git bridge

Git is an **adapter**, not the engine. The bridge uses the `git` command line (plumbing only), so it needs
`git` on `PATH` and never links Git's code.

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

**Not carried over**, and reported when it happens: submodules (gitlinks), paths AgentGit refuses to record
(non-UTF-8, control characters, backslashes, and any `.git`/`.ag` component — a repository can contain
such trees via plumbing, and checking them out would let it plant hooks), tags, notes, signatures,
committer identity and timezone offsets. The command prints each skipped path.

## Export a stream to Git

```bash
ag git export main /path/to/new-or-existing/git/repo --branch main
```

Uses `git fast-import`. The destination is `git init`-ed if missing. An identity such as `user:alice`
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
