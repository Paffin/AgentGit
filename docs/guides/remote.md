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
export AG_TOKEN=agt_…                                   # a credential minted on the server
ag clone https://git.example.com:8443/acme/web web      # http:// works for a local or TLS-terminated server
cd web
ag remote whoami                                        # who the server thinks you are, and your roles
```

A remote is `<server>/<owner>/<name>`; `ag clone` records it as `origin` in `.ag/remotes.json` (no secrets in it).
Where the credential comes from, in order: the variable named by `--token-env`, a file you stored with
`--token-stdin` (`.ag/credentials/<remote>`, mode 0600 — `ag` **refuses** a credential file other users can
read), `AG_TOKEN_<REMOTE>`, then `AG_TOKEN`. For TLS with a private CA add `--ca-file ca.pem`. To act as an agent
run, export `AG_RUN=<run id>` (the credential must belong to that agent).

```sh
echo "$TOKEN" | ag clone --token-stdin https://… web        # store it instead of keeping it in the environment
ag remote add upstream https://other.example.com/acme/web --token-env UPSTREAM_TOKEN
ag remote list
```

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
