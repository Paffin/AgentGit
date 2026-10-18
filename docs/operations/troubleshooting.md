# Troubleshooting

Start with `ag verify` and, for an integration, `ag --json op show <operation-id>`. For an integration that is blocked or
refused, [Troubleshooting a blocked integration](../tutorials/troubleshooting-blocked-integration.md) reproduces every
message below with commands that are run by the tests, and explains the cases that surprise people (a check that "passed" and
still does not count, approvals that disappear).

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
| `transient metadata failure (safe to retry): the database is busy: database is locked` | SQLite: another process held the write lock for more than 30 s; find it (a hung `ag`, a backup, a debugger) and run the command again |
| `key file … is readable by other users; run chmod 600` | `chmod 600 .ag/keys/*.json` |
| `database schema version N is newer than supported` | upgrade `ag` |
| `cannot run git` | install git (only the Git bridge needs it) |

## PostgreSQL and S3

| You see | Meaning | Do |
|---|---|---|
| `the environment variable AG_PG_URL is not set; it must hold the PostgreSQL connection URL (named by storage.meta.url_env …)` (or the S3 credential variables) | the configuration only names the variables; they must be set in the environment of every `ag` process | export them (the message names which) |
| `storage.meta.url contains a password …` | a literal URL with a password is refused so that secrets stay out of `.ag/config.json` | put the URL in an environment variable and use `url_env` |
| `PostgreSQL schema '…' holds no repository (was it dropped, or does the connection URL point at the wrong database?)` | opening never creates a schema | check `AG_PG_URL` and `storage.meta.schema`; restore the schema if it was dropped |
| `the PostgreSQL schema '…' already holds a repository` | `ag init` refuses to initialize over one | use another `schema`, or open the existing one |
| `transient metadata failure (safe to retry): … lock timeout (SQLSTATE 55P03)` | a write waited 30 s for the repository's write lock: another writer is busy or stuck | run the command again; find a long transaction with `SELECT * FROM pg_stat_activity WHERE state = 'idle in transaction'` |
| `lost the connection to the metadata database: …` | the connection died before a commit was requested; nothing was committed | run the command again |
| `lost the connection … while committing (the outcome is unknown)` | the connection died while the commit was in flight; it may or may not have been applied | do **not** repeat blindly: `ag op show <operation-id>` (or re-run `ag integrate <proposal>`, which replays a recorded result) |
| `database schema version N is newer than supported version M` | the schema was migrated by a newer `ag` | upgrade `ag`; there is no downgrade |
| `error performing TLS handshake: … UnknownIssuer` / `invalid peer certificate` | `sslmode=verify-full` and the server's CA is not trusted | add `sslrootcert=/path/to/ca.pem` to the URL (or use a certificate from a public CA) |
| `HTTP 403 SignatureDoesNotMatch` / `InvalidAccessKeyId` (S3) | wrong access key or secret | check the credential variables named in `storage.objects` |
| `HTTP 403 RequestTimeTooSkewed` (S3) | this machine's clock is far from the service's | fix NTP |
| `HTTP 404 NoSuchBucket` / `HTTP 301 PermanentRedirect` (S3) | the bucket does not exist, or the region/endpoint does not match it | create the bucket; fix `region` / `endpoint` |
| `HTTP 403 AccessDenied` (S3) | the credentials lack `s3:ListBucket` / `GetObject` / `PutObject` / `DeleteObject` on the prefix | grant them |
| `… (gave up after N attempt(s))` (S3) | the service kept failing with 5xx/timeouts through all retries | check the service; run the command again |
| `this repository keeps its metadata in PostgreSQL … which `ag backup` cannot copy consistently` | `ag backup` does not back up PostgreSQL | [Durability](durability.md#postgresql-and-s3) |
| `unable to open database file` / `Not a directory` for `.ag/meta.db` or `.ag/store` on an older `ag` | the repository uses PostgreSQL/S3 and this binary predates the `storage` section; the placeholder stops it from creating a local copy | upgrade `ag` |

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
| `dangling_pin` / `dangling_proposal_head` / `dangling_workspace_ref` | a pin, an open proposal or a workspace names a changeset the object store cannot produce (`ag gc` refuses to run until it is fixed) | restore the object (`ag backup`/mirror) or the metadata; remove the pin/workspace if it is no longer wanted |
| `audit_chain_broken` | an audit entry was altered or removed | treat as tampering; compare with the audit head hash you recorded |

## `ag gc` says "aborted: reachable … is unreadable"

GC refuses to delete anything when the mark phase can't read something it must keep. Run `ag verify`,
repair or restore the named object, then rerun. This is by design.

## Still stuck

Open an issue using the bug template with `ag --version`, the exact commands, and the output of `ag verify`
and `ag --json op show <id>`. For suspected vulnerabilities see [SECURITY.md](../../SECURITY.md).
