# Migrating a Git repository

AgentGit does not ask you to give up Git. The [Git bridge](../guides/git-bridge.md) reads a Git repository
into an AgentGit stream (`ag git import`) and writes a stream back out as a Git branch (`ag git export`), so you can
move over in one step, or run both side by side while you find out whether AgentGit's policy, leases and agent
workflow are worth it for you. This tutorial does it on a small repository that it builds first, so every command
below really runs ([how the tutorials are tested](README.md#how-these-tutorials-are-tested)); replace `legacy` with
the path to your own repository.

You need `ag`, `git` on your `PATH` (the bridge uses Git's plumbing commands and never links Git's code), and a shell.

<!-- doctest-needs git -->

## What carries over, and what does not

| Carried over | **Not** carried over |
|---|---|
| every commit, with the **same parents in the same order** (merges included) | submodules (gitlinks) |
| file contents, executable bits, symlinks | paths AgentGit refuses to record: non-UTF-8 names, control characters, backslashes, any `.git` or `.ag` component |
| author (as `user:<email>`), commit time, message, plus a trailer `Imported-From: git <sha>` | tags, notes, signatures, the committer's identity, time-zone offsets |

Skipped paths and submodules are **reported** while importing; nothing is dropped silently. Import never rewrites an
existing stream: the target stream must not have history yet. Git LFS is not supported (and not tested): a pointer file
would be imported as the small text file it is. Large repositories: the whole history is imported as one verified
operation; we have not measured it on big repositories, so try your own on a copy first.

## 1. A repository to migrate

This is a Git repository with a branch, a merge, an executable file and three authors' worth of work (one author, to be
honest). Skip this step if you already have one.

<!-- doctest -->
```bash
export AG_IDENTITY=user:alice
git init -q -b main legacy
git -C legacy config user.name "Dana Developer"
git -C legacy config user.email "dana@example.com"
printf 'fn main() {}\n' > legacy/main.rs
printf '#!/bin/sh\necho build\n' > legacy/build.sh
chmod +x legacy/build.sh
git -C legacy add -A
git -C legacy commit -q -m "Initial commit"
git -C legacy checkout -q -b feature
printf 'fn main() { println!("hi"); }\n' > legacy/main.rs
git -C legacy commit -q -am "Print a greeting"
git -C legacy checkout -q main
printf '# legacy\n' > legacy/README.md
git -C legacy add -A
git -C legacy commit -q -m "Add README"
git -C legacy merge -q --no-ff feature -m "Merge feature"
git -C legacy rev-list --count HEAD
git -C legacy rev-list --merges --count HEAD
```

<!-- expect -->
```text
4
1
```

Four commits, one of them a merge.

## 2. Import it

Create an AgentGit repository (in a new directory: the Git repository stays untouched) and import into its default
stream `main`. Then `ag sync` writes that stream's files into the working directory.

<!-- doctest -->
```bash
mkdir migrated && cd migrated
ag init --namespace acme/legacy
ag git import ../legacy --rev HEAD --stream main
ag sync
ls
ag log main
```

<!-- expect -->
```text
Imported 4 commit(s), 4 distinct file content(s) into stream 'main' (head ...).
Fast-forwarded to ....
README.md
build.sh
main.rs
... user:dana@example.com  (merge of ... + ...)
    Merge feature
...
    Initial commit
```

The merge is still a merge, with both parents (the two commits it joins can come in either order, because they carry
the same second-resolution timestamp). Dana's email became her AgentGit identity, `user:dana@example.com`. Every
changeset's message ends with the commit it came from, which is how you pair the two histories later:

<!-- doctest -->
```bash
ag log main | grep -c 'user:dana@example.com'
ag show main | grep -c 'Imported-From: git'
ag verify
```

<!-- expect -->
```text
4
1
...
Repository is healthy.
```

`ag verify` re-hashed every stored object and cross-checked the metadata: the imported repository is internally
consistent.

## 3. Prove nothing was lost

The bridge's promise is that for histories without skipped paths, **import followed by export reproduces the same Git
tree for every commit**. Check it on yours instead of trusting it: export the stream to a fresh Git repository and
compare the trees of the two heads. Commit ids will *differ*, because identities and timestamps are mapped.

<!-- doctest -->
```bash
ag git export main ../roundtrip --branch main
test "$(git -C ../legacy rev-parse 'HEAD^{tree}')" = "$(git -C ../roundtrip rev-parse 'HEAD^{tree}')" && echo "same tree"
test "$(git -C ../legacy rev-parse HEAD)" != "$(git -C ../roundtrip rev-parse HEAD)" && echo "different commit ids"
git -C ../roundtrip rev-list --count HEAD
git -C ../roundtrip rev-list --merges --count HEAD
```

<!-- expect -->
```text
Exported 4 commit(s) of 'main' to branch 'main' in ../roundtrip
same tree
different commit ids
4
1
```

The same tree id means identical file contents, names and modes at the head, and the counts show the same shape (four
commits, one merge). For a
real repository, run the same comparison on a few older commits too (`git rev-parse <commit>^{tree}` on both sides; the
`Imported-From:` trailer tells you which commit pairs with which).

## 4. Turn the rules on

Nothing has changed for the people who still use Git. In AgentGit, the imported stream is an ordinary stream: give it a
policy and the next change goes through a proposal.

<!-- doctest -->
```bash
printf '{ "approvals": { "min": 1, "forbid_self_approval": true } }\n' > ../policy.json
ag policy set ../policy.json
printf '# legacy\n\nMigrated to AgentGit.\n' > README.md
ag snapshot -m "Note the migration"
P=$(ag -q propose -t "Note the migration")
ag integrate "$P"                                   # expect-exit 3
ag --as user:bob proposal review "$P" --approve
ag integrate "$P"
```

<!-- expect -->
```text
Policy of 'main' is now version 1 (hash ...)
Snapshot ... in workspace 'default'
Blocked (policy v1); the stream was not changed:
  - approvals (default): 0 of 1 required
...
Recorded 'approve' by user:bob on ...
Integrated (fast-forward): stream is now at ... (version 2, was ...), 1 file(s) changed.
```

From here the loop is the one in [Your first agent-safe repository](first-agent-safe-repo.md): add CI as a trusted
issuer, register agents, hand out leases.

## 5. Living with both for a while

The bridge is **one-shot in each direction, not a live mirror**. A workable arrangement while you evaluate:

- Git stays the system of record. When your Git history moves, import it again into a **new** stream (an import never
  touches an existing one) and let agents work from that stream with AgentGit's policy and leases.
- Export what comes out as a branch for people to look at.

<!-- doctest -->
```bash
printf 'fn extra() {}\n' > ../legacy/extra.rs
git -C ../legacy add -A
git -C ../legacy commit -q -m "Upstream change"
ag git import ../legacy --rev HEAD --stream sync-1
ag stream list
ag git export main ../legacy --branch agentgit-main
git -C ../legacy branch
```

<!-- expect -->
```text
Imported 5 commit(s), 5 distinct file content(s) into stream 'sync-1' (head ...).
main ...  (version 2)
sync-1 ...  (version 1)
Exported 5 commit(s) of 'main' to branch 'agentgit-main' in ../legacy
  agentgit-main
  feature
* main
```

Be clear about what that branch is: a **parallel history** with the same trees and different commit ids. It shares no
commits with your original `main`, so Git cannot fast-forward or merge one into the other; bringing a change across is a
patch (`git diff`/`git apply`), a cherry-pick of the content, or a decision to make the exported branch the new `main`.
That is the price of the one-shot bridge. If you decide to move for good, the clean cut is: freeze Git, import, verify
as in step 3, export once to the Git host you keep for read-only mirrors, and switch.

## Where to go next

- [The Git bridge reference](../guides/git-bridge.md): what the import maps and the round-trip guarantee's test.
- [Concepts](../concepts.md) for the vocabulary (changeset, stream, proposal) and [AgentGit compared](../comparison.md)
  for how this differs from Git, pull requests and merge queues.
- [Run a team server](team-server.md) to share the migrated repository.
