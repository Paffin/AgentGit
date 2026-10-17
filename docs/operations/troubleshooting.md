# Troubleshooting

Start with `ag verify` and, for an integration, `ag --json op show <operation-id>`.

## Exit codes at a glance

`0` ok · `1` error · `2` usage error · `3` **blocked** (provide the missing thing, run the same command
again) · `4` **refused** (conflict / denied / invalid; the stream was not changed).

## Integration messages

| You see | Meaning | Do |
|---|---|---|
| `Blocked … has no trusted passing attestation for this exact tree` | policy requires a validation and none matches the *candidate* tree | `ag validate run <proposal> --check <name> --key <issuer> -- <cmd>`; if the stream moved since, validate again |
| `approvals (default): 0 of 1 required` | not enough eligible approvals **of the current head** | `ag proposal review <id> --approve` by someone eligible (not the author, not an agent unless allowed) |
| `agent-authored change has no signed provenance` | policy requires it | `ag provenance attach @ws --agent <name>` |
| `exclusive lease on '…' held by … (held_by_other / stale_or_missing_token)` | someone holds the scope, or you did not pass your current token | wait / `ag lease list`; `ag integrate … --fencing-token N` |
| `Denied … actor not allowed to integrate` | agents can't integrate unless `agent_autonomy` allows it | have a human/service integrate |
| `Denied … check 'x' FAILED on this exact tree` | a trusted issuer recorded a failure for this tree | fix the code, update the proposal (new head → new tree) |
| `Conflict … f.txt (content conflict …)` | both sides edited overlapping or **adjacent** lines | `ag sync`, resolve markers, `ag snapshot`, `ag proposal update` |
| `Invalid: … criss-cross history` | two equally good merge bases | merge the stream into your work by hand (`ag sync` handles one base only) |
| `Invalid: … unrelated` | the proposal shares no history with the stream | rebase your work onto the stream |
| `Invalid: the proposal was updated after this integration started` | a stale operation | run `ag integrate <id>` again (new attempt) |
| `too much contention: stream changed 8 times while integrating` | extreme write contention | retry |
| `idempotency key '…' was already used for a different request` | same `--key` for another proposal/head | use a new key |

## Workspaces

| You see | Do |
|---|---|
| `the directory has N uncommitted change(s); snapshot them first` | `ag snapshot` (or discard) before `ag sync` |
| `unresolved conflict markers in: …` | edit the files, or `ag snapshot --force` if the markers are intentional |
| `a conflicted sync is pending` | resolve + `ag snapshot` first |
| `cannot record 'x': path contains a forbidden character … (add it to .agignore)` | rename the file or list it in `.agignore` |
| `unsafe path: … is a symlink; refusing to write through it` | the directory contains a symlink where AgentGit needs a directory; remove it |
| `symlink '…' escapes the repository root` | the tree contains an absolute or climbing symlink; it will not be materialized |

`.agignore` (in the working directory): one glob per line, `#` comments; a pattern without `/` matches at any
depth. `.ag` and `.git` are always ignored.

## Environment

| You see | Do |
|---|---|
| `not an AgentGit repository (or any parent …)` | run inside a repository or pass `-C <dir>` |
| `metadata database error: database is locked` | another process holds the write lock for more than 30 s; find it (a hung `ag`, a backup, a debugger) |
| `key file … is readable by other users; run chmod 600` | `chmod 600 .ag/keys/*.json` |
| `database schema version N is newer than supported` | upgrade `ag` |
| `cannot run git` | install git (only the Git bridge needs it) |

## `ag verify` issue kinds

| Kind | Meaning | Remedy |
|---|---|---|
| `corrupt_object` / `bad_blob` | an object's bytes no longer hash to its ID | restore from `ag backup`/mirror; re-publishing identical content also repairs it |
| `missing_object` | listed object vanished | restore |
| `unreadable_changeset` | a published changeset can't be decoded | restore |
| `index_mismatch` | the index disagrees with the object | `ag reindex`; if it persists, restore |
| `dangling_parent` | a published changeset has an unpublished parent | restore the metadata from backup |
| `stream_head_unpublished` | a stream points at an unpublished changeset | restore the metadata from backup |
| `history_not_derivable` | the history below a head does not apply cleanly | restore |
| `stale_derived_tree` | cached tree disagrees with history | `ag reindex` (the cache is disposable) |
| `reflog_mismatch` | a reflog does not end at its stream's head/version | restore; indicates tampering or a bug |
| `audit_chain_broken` | an audit entry was altered or removed | treat as tampering; compare with the audit head hash you recorded |

## `ag gc` says "aborted: reachable … is unreadable"

GC refuses to delete anything when the mark phase can't read something it must keep. Run `ag verify`,
repair or restore the named object, then rerun. This is by design.

## Still stuck

Open an issue using the bug template with `ag --version`, the exact commands, and the output of `ag verify`
and `ag --json op show <id>`. For suspected vulnerabilities see [SECURITY.md](../../SECURITY.md).
