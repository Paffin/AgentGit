# Git to AgentGit

If you know Git, this page maps what you do to what you type here, and says where the idea is different. It lists only
what exists; where AgentGit has no equivalent it says so. Every command is in the [CLI reference](reference/cli.md); the
vocabulary is in [Concepts](concepts.md) and the [glossary](glossary.md). To move an existing repository, see
[Migrating from Git](tutorials/migrating-from-git.md).

## The daily loop

| In Git | In AgentGit | What is different |
|---|---|---|
| `git init` | `ag init` (`--preset team` stores a [policy preset](guides/policy.md#start-from-a-preset)) | The repository is `.ag/` (a SQLite file and content-addressed objects); there is no `.git`. |
| `git clone URL` | `ag clone http://host:8080/owner/name` | Clones from an `ag-server`. Log in once with `ag login` and no token handling is needed. |
| `git status` | `ag status` | Also shows your proposals, leases, task and remote at a glance. |
| `git add` + `git commit` | `ag snapshot -m "message"` | There is **no staging area**: a snapshot records the whole working directory (minus what `.agignore` lists). A snapshot is private to your workspace until you propose it. |
| `git push` (then "open a pull request") | `ag push -t "title"` (server), or `ag propose -t "title"` (local) | Publishing and asking for integration are one step. The result is a **proposal**. Nothing you push can move a stream. |
| pull request / merge request | proposal | Reviews are attached to the exact version they looked at: when the proposal changes, earlier approvals stop counting. |
| clicking "Merge" | `ag integrate PROPOSAL` (local), `ag remote integrate PROPOSAL` (server) | The **policy** decides, in the same transaction that moves the stream. Exit code 0 landed, 3 blocked (something you can provide), 4 refused (conflict or denial). |
| "why can't I merge?" | `ag explain [PROPOSAL]` | Each reason in plain words, with the exact commands that fix it. On a server: `ag remote explain PROPOSAL`. |
| `git fetch` | `ag fetch` | |
| `git pull` | `ag pull` | Fast-forward only; it never creates a merge by itself. To bring new work into a workspace that has its own changes: `ag sync`. |
| `git log` | `ag log` | 20 entries by default; `-n N` for more. |
| `git show REV` | `ag show REV` | |
| `git diff A B` | `ag diff A B` | |
| `git blame FILE` | `ag blame REV:FILE` | Follows renames. |
| `git ls-tree`, `git show REV:path` | `ag ls REV`, `ag cat REV:path` | |
| `git checkout REV -- .` into a new place | `ag checkout REV DIR` | Writes a revision's files into a new directory. |
| `git remote add origin URL` | `ag remote add origin URL` (a clone sets `origin` itself) | |
| a credential helper, a personal access token | `ag login URL --token-file FILE` (or `--token-stdin`) | One saved credential per server, in a private file beside your user configuration (`ag config path`). `AG_TOKEN` still wins when set. |

## Branches

| In Git | In AgentGit | What is different |
|---|---|---|
| branch | **stream** | A named line of integration with a head and a version. A stream moves only by compare-and-swap, and every move is in an append-only reflog (`ag stream reflog`). |
| `git switch -c topic` | `ag workspace create topic --dir ../topic` | Your *own* line of work, checked out in its own directory, so people and agents never trample each other. A workspace is not published until you propose it. |
| `git merge main` into your branch | `ag sync` | Brings the stream's head into your workspace: clean parts merge, overlapping ones get `<<<<<<<` markers. Resolve, `ag snapshot`, `ag proposal update ID`. |
| merge conflict | exit code 4, then `ag explain` | The stream is untouched. The merge is deterministic and rename-aware; it does not guess between two edits of the same lines. |
| `git branch -a` | `ag stream list` | |

## What Git leaves to the platform, AgentGit does in the tool

| You want | In Git | In AgentGit |
|---|---|---|
| "changes need an approval" | branch protection on the hosting service | `approvals` in the **policy**; `ag proposal review ID --approve` |
| code owners for sensitive files | `CODEOWNERS` on the hosting service | `protected_paths` with their own approval counts |
| "tests must pass" | a status check from CI | `required_validations`: a **signed attestation** from a trusted issuer, about the *exact tree* that would land. `ag validate run ID --check tests --key ci -- <command>` |
| "an AI agent may touch only docs" | nothing; convention | `agent_autonomy` (allowed paths, file limit, whether it may land alone) and `ag agent`, leases and signed provenance |
| "two agents must not edit the same area" | merge conflicts afterwards | **leases**: `ag lease acquire 'src/**' --exclusive` hands out a fencing token that integration checks |
| a starting point instead of writing rules | a template repository | `ag policy preset list` (solo, team, agents, strict); `ag policy explain` reads any policy back in words |
| an audit trail | the hosting service's log | `ag audit` verifies a tamper-evident chain; `ag trace` shows what happened to an object, proposal or run |

## Things with no equivalent here

- **Rewriting history** (`rebase`, `commit --amend`, `reset --hard`, `filter-repo`). History is immutable; you add changesets, and
  `ag sync` merges rather than rebases.
- **Staging parts of a file** (`git add -p`). A snapshot records the directory as it is.
- **`cherry-pick`, `stash`, `bisect`, submodules, hooks.** Not provided. A workspace takes the place of a stash.
- **Tags as release markers.** `ag pin NAME REV` keeps a revision from garbage collection, which is a different thing.
- **Reading Git's formats directly.** `ag git import` and `ag git export` move history between the two (they need the `git`
  command installed).

## Words

| Git | AgentGit |
|---|---|
| commit | **changeset** (a delta against its first parent; the resulting state is a **tree**) |
| branch | **stream** |
| working tree / branch checkout | **workspace** |
| pull request | **proposal** |
| merge | **integration** (fast-forward or deterministic three-way merge) |
| CI status | **attestation** (signed, about one tree and one policy version) |
| branch protection | **policy** |
