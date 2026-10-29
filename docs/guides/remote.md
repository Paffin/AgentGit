# Working with a server: clone, fetch, pull, push — and offline

A local AgentGit repository is a complete, embedded repository: every command in the [CLI
reference](../reference/cli.md) works without a network. `ag` talks to an [`ag-server`](../operations/deployment.md)
over gRPC when you ask it to. The model is deliberately small:

- your **local default stream mirrors the server's stream**. `pull` only ever fast-forwards it; you never
  move the server's head yourself;
- changes travel to the server as **proposals** (`push`), and land there through `integrate` under the
  server's policy, approvals and validations — exactly like a proposal opened on the server.

## Connecting

```sh
ag login https://git.example.com:8443                   # paste a credential minted on the server
ag auth status --check                                 # verify saved connections without printing credentials
ag clone https://git.example.com:8443/acme/web web      # http:// works for a local or TLS-terminated server
cd web
ag remote whoami                                        # who the server thinks you are, and your roles
```

A remote is `<server>/<owner>/<name>`; `ag clone` records it as `origin` in `.ag/remotes.json` (no secrets in it).
Where the credential comes from, in order: the variable named by `--token-env`, a file you stored with
`--token-stdin` (`.ag/credentials/<remote>`, mode 0600 — `ag` **refuses** a credential file other users can
read), `AG_TOKEN_<REMOTE>`, `AG_TOKEN`, then a saved `ag login` for that server. For TLS with a private CA add `--ca-file ca.pem`. To act as an agent
run, export `AG_RUN=<run id>` (the credential must belong to that agent).

```sh
echo "$TOKEN" | ag clone --token-stdin https://… web        # store it instead of keeping it in the environment
ag remote add upstream https://other.example.com/acme/web --token-env UPSTREAM_TOKEN
ag remote list
```

`ag auth status` works outside a repository and without a network. It lists saved identities and expiry times,
not token values, and distinguishes a cached login from one verified online. Use `ag auth status SERVER --check`
to check one connection; `--timeout 5` bounds the check in seconds and `--ca-file ca.pem` supports a private CA.
An expired, rejected or unreachable connection returns exit code 1. The command never changes saved credentials
and does not list environment or remote-specific credentials. Run `ag login SERVER` to replace a saved login,
or `ag logout SERVER` to forget it. For automation, `AG_TOKEN` and `ag login --token-stdin` remain available.

## The loop

```sh
ag pull                                  # fetch, fast-forward the local stream, bring the workspace up to date
# ... edit files ...
ag snapshot -m "Rename the config loader"
ag push -t "Rename the config loader"    # upload what the server lacks, open a proposal
ag remote prepare prop_…                 # what would integrating do? what is missing? (exit 3 = blocked, 4 = conflict)
ag remote review prop_… --approve        # a reviewer, from anywhere
ag remote integrate prop_…               # an integrator
ag pull                                  # the merged result comes back
```

- `ag fetch` downloads new changesets and blobs (only what you lack: the server computes `want`/`have`, you
  get them parents first), verifies every object by its content hash, and publishes them locally.
- `ag pull` = `fetch` + fast-forward of the local stream + a workspace sync. If the workspace has changes the
  merge cannot combine, you get the same conflict markers as `ag sync`, and exit code 4.
- `ag push` uploads only objects the server reports missing, then creates the proposal with an idempotency
  key derived from the head, so repeating a push that timed out cannot create a second proposal.
  It refuses while the working directory has uncommitted changes (`ag snapshot` first).
- Everything else about a proposal is `ag remote …`: `proposals`, `proposal`, `prepare`, `review`, `integrate`,
  `close`, `streams`, `events [--follow]`, `policy`, `log`, `cat`, `blame`, `whoami`. They mirror the local
  commands of the same names but act on the server; `--json` works everywhere. Listings are complete: `proposals`
  and `events` read every page the server has, and `log -n N` asks for as many pages as `N` needs. `events
  --follow --json` prints one compact JSON object per line (NDJSON), so a script can read it line by line;
  `remote cat` writes the file's raw bytes. `remote blame --max-commits N` bounds how far back blame looks (the
  server's own limit applies by default, and the output says when blame stopped there).
- Every call to a server has a deadline: 120 seconds, or `AG_TIMEOUT` seconds (a whole number greater than zero;
  anything else is refused, not ignored). A push sends its objects in batches of at most 32 MiB, each call with the
  full deadline, so the deadline bounds a batch and not the whole transfer over a slow link. A download that stops
  in the middle ends at the same deadline instead of waiting for ever.
- `ag remote remove NAME` deletes the stored credential first; if it cannot, the remote is kept and the error
  names the file.

## Private review drafts

On development servers advertising `proposals.review_drafts`, save review notes to your
account and resume them in the browser or SDK. Read the draft first, then pass its exact
version when saving or clearing it. A missing draft starts at version zero; a cleared draft
keeps its later version. Another device's edit produces a conflict instead of losing notes.

```sh
ag remote review-draft get prop_…
ag remote review-draft save prop_… --head CHANGESET_ID --decision comment --body-file notes.md --expected-version 0
ag remote review-draft clear prop_… --expected-version 1
```

Use the version actually returned by your preceding command, not the example values above.
The head records the revision you inspected. Saving does not submit a review, grant approval,
or notify reviewers. Only the current account can read its draft through the API; server
administrators and backups can still contain these notes. Files use UTF-8 and retain line
breaks, with a 64 KiB limit. `--json` returns the draft and its version for scripts.

## Private comment drafts

On development servers advertising `proposals.comment_drafts`, writers can save a general
proposal comment privately before posting it. These notes are separate from review-decision
drafts. Read the draft first and use its returned version for every save or clear.

```sh
ag remote comment-draft get prop_…
ag remote comment-draft save prop_… --head CHANGESET_ID --body-file notes.md --expected-version 0
ag remote comment-draft clear prop_… --expected-version 1
```

The versions above are examples; use the values returned by your preceding command. The head
records the revision you inspected. Saving never posts a comment or sends a notification.
Drafts remain available on closed proposals and only the current account can read them through
the API; server administrators and backups can still contain them. UTF-8 files retain their
line breaks and are limited to 16 KiB. `--json` includes the version, including after clearing.

## Private file drafts

Development servers advertising `files.drafts` can keep private replacement bytes for an
existing file, bound to the exact base changeset you inspected. Saving a draft does not create
a changeset, update a stream, or change a proposal.

```sh
ag remote file-draft list --stream main
ag remote file-draft get --stream main --path src/config.json
ag remote file-draft save --stream main --path src/config.json --base CHANGESET_ID --content-file edited.json --expected-version 0
ag remote file-draft get --stream main --path src/config.json --output restored.json
ag remote file-draft clear --stream main --path src/config.json --expected-version 1
```

Use the version returned by `get` for saves and clears, including the version of a cleared
draft. Content files are binary-safe and limited to 4 MiB. Commands show metadata; bytes are
written only when `get --output` is explicit, and that option replaces the named local file.
The original file type and executable bit come from the immutable base. Each account can
keep at most 20 active file drafts per repository. Drafts are private through the API;
administrators and backups can still contain them.

## Offline

Nothing about a local repository needs the server:

```sh
ag snapshot -m "work on the train"       # local changesets, local history
ag log; ag diff; ag blame src/lib.rs     # all local
# ... later, with a connection:
ag pull                                  # first catch up: the merge happens in your workspace
ag push -t "Work on the train"           # then publish
```

If the server's stream moved while you were away, `push` still works — the server merges the proposal at
integration time like any other, and reports a conflict then if the edits overlap. Pulling first lets you
resolve conflicts locally, with markers, before anyone is waiting.

**Do not run `ag integrate` on the mirrored stream of a clone.** It would advance your local `main` past the
server's, and `ag pull` will then refuse to fast-forward ("it has local history the server lacks"). Land
changes on the server; use `ag integrate` only in a repository that is itself the source of truth.

## Limits (today)

- One stream per command (`--stream`, default: the repository's default stream); no shallow or partial
  clones, no submodules.
- The CLI speaks gRPC only. The HTTP/JSON API and the [SDKs](../protocol/agwp-v1.md) cover the same operations
  for other tools.
- A crash in the middle of `fetch` leaves already-downloaded objects in the local store (harmless; the next
  fetch skips them) but does not move the tracking reference, so it is simply retried.
- The Rust client crate `ag-client` is what `ag` uses; it retries only errors the server marks retryable,
  honouring `retry_after_ms`.

## Understand a blocked change

In a clone, run `ag remote explain PROPOSAL` to get each reason, why it matters and the next commands to run.
It reads the server's current preparation without requesting validation jobs or changing the proposal.
Use `ag --json remote explain PROPOSAL` in scripts: exit 0 means ready, 3 means a requirement is missing,
and 4 means refused, conflicted or already closed. The result includes the candidate tree and policy version.
An explanation is a snapshot; integration always checks the current policy and candidate again.

## Server-side workspaces

Use `ag remote workspace` for an isolated workspace on the server, without a local checkout:

```bash
ag remote workspace list
ag remote workspace create review-fix --stream main --from <proposal-head>
ag remote workspace show review-fix
ag --json remote workspace sync review-fix
```

A conflicting sync exits with code 4 and leaves the workspace unchanged. Its JSON includes the
workspace `head`, the exact `stream_head` inspected, and every conflict. For file/directory conflicts,
`resolution_scope.paths` lists the candidate paths and `resolution_scope.complete` says whether that
list is complete. An incomplete list is not permission to discard the omitted files. Read both versions
and choose every change explicitly; the CLI does not select a side, generate deletions, or update a proposal.

Save your choices as a JSON array of `FileChange` objects. `content` is standard base64 (including an
explicit empty string for an empty file); alternatively use `blob_id` to preserve existing binary data.
Carry over `file_type` and `executable` when choosing an existing version. A deletion has only `path`
and `op`:

```json
[
  {"path": "clash", "op": "put", "content": "ZmlsZQ==", "file_type": "regular", "executable": false},
  {"path": "clash/a", "op": "delete"},
  {"path": "clash/b", "op": "delete"}
]
```

Apply that file against the `stream_head` from the sync you inspected:

```bash
ag --json remote workspace sync review-fix --resolutions choices.json --expected-stream-head <stream-head>
ag remote workspace delete review-fix
```

`--resolutions` requires `--expected-stream-head`. The CLI refuses application unless the server
advertises both guarded sync and structural resolution support. A changed stream head rejects the
operation; inspect a fresh sync result before deciding again. Deleting a workspace removes that
server-side workspace, not its proposal or the stream.
