# Running agents against a shared repository

AgentGit treats an agent as an **untrusted collaborator with an identity**: it can propose, claim work and
leave a signed account of what it did, but it cannot land code unless policy says so, and a hijacked agent
can still only *propose*. This guide shows the full loop. Every command below is exercised by
[`examples/quickstart.sh`](../../examples/quickstart.sh) and `crates/ag-cli/tests/cli.rs`.

## 1. Identity: agent and run

```bash
ag agent register refactor-bot          # creates agent:refactor-bot and its Ed25519 key
ag --json agent start refactor-bot      # → {"run_id":"run_…","identity":"agent:refactor-bot#run_…"}
export AG_IDENTITY='agent:refactor-bot#run_…'
```

- The **agent** is the long-lived principal (`agent:refactor-bot`); each execution is a **run** with its own
  suffix. Audit entries, proposals and leases are attributed to the run; rules that compare people
  (self-approval, lease ownership) compare the principal.
- `ag agent heartbeat <run>` from long runs; `ag agent reap --timeout 10m` abandons silent runs and
  releases their leases. `ag agent finish <run>` ends a run cleanly (also releases leases).
- `ag agent disable <name>` stops an agent from starting new runs.
- The agent's private key is `.ag/keys/<name>.json` (`0600`). **Keep it out of any environment that runs
  untrusted code.** Move it with `AG_KEY_DIR`.

## 2. Work: tasks and dependencies

```bash
ag task create "Design the API" --scope 'api/**'
ag task create "Implement the API"
ag task depend <implement> <design>     # implement waits for design; cycles are refused
ag task ready                           # open tasks whose dependencies are done
ag agent start refactor-bot --task <id> # claims the task; fails if it is blocked or owned by someone else
ag task done <id>                       # only the owner can finish it
```

### Bound retries without losing the task (development)

```sh
ag task create "Repair the build" --max-attempts 3
ag task attempts <task-id>
ag task attempts <task-id> --max 5 --expected 3
# The same policy on a configured server:
ag task attempts <task-id> --remote origin --max 5 --expected 3
```

The server counts every run admitted for the task, including failed, interrupted and currently
running attempts. A replayed start counts once. The count survives orchestrator restart.
An explicit limit keeps a failed task available for operator recovery; extending it preserves
instructions, dependencies and all run records. After execution starts, an operator must change
the limit. Zero retains the legacy server behavior and the orchestrator's repository retry policy.
An explicit limit includes temporary failures too. Lowering it does not stop an active run.

This requires metadata schema 8 and a server advertising `tasks.attempt_limits`. Back up before
upgrade; older binaries need the pre-upgrade backup. See [ADR 0033](../adr/0033-task-attempt-limits.md).

## 3. Coordination: leases and fencing

```bash
ag lease acquire 'src/auth/**' --ttl 30m                 # soft: a signal, never blocks anyone
ag lease acquire 'src/auth/**' --ttl 30m --exclusive     # prints a fencing token, e.g. 7
ag lease extend <lease> --ttl 30m                        # token stays the same
ag lease list                                            # active leases (--all for history)
```

- **Soft leases** are advisory: acquiring one over someone else's reports the overlap as a warning.
- **Exclusive leases** are refused while another principal holds an overlapping *exclusive* lease, and are
  **enforced at integration**: while it is active, a change touching its scope can only land if its author is
  the holder *and* the integration presents the lease's current token:

  ```bash
  ag integrate <proposal> --fencing-token 7
  ```

- Tokens come from one strictly increasing counter. If a holder stalls, its lease expires and someone else
  acquires the scope with a higher token; when the stalled holder wakes up, its old token (and its identity)
  no longer pass. **Expiry never deletes the changes an agent already proposed** — it only stops a stale
  holder from landing into a scope that is now someone else's.
- Overlap detection compares literal path prefixes of the globs; it is conservative (may report an overlap
  that is not real, never misses one).

## 4. Produce: workspace, snapshot, propose

```bash
ag workspace create refactor --dir ../ws-refactor        # isolated directory, starts at the stream head
# …edit files in ../ws-refactor…
ag snapshot -m "Friendlier greeting" --workspace refactor
ag propose @refactor -t "Friendlier greeting"            # publishes (verifies everything) + opens a proposal
```

`ag predict` shows overlapping work *before* it collides: proposals touching the same paths, the stream
having moved under a proposal, proposals inside someone else's lease, in-progress tasks with overlapping
scopes. It is a path-level early warning; `ag preview <proposal>` gives the definitive merge answer.

## 5. Account for it: provenance

```bash
ag provenance attach @refactor --agent refactor-bot --run run_… \
   --model my-model --prompt-file prompt.txt --rationale "…" --check "cargo test" --input "issue #42"
ag provenance show main
```

The record (model, SHA-256 of the prompt — never the prompt —, parameters, checks the agent ran, sources,
rationale) is signed with the agent's key and stored **beside** the changeset, so code history stays
immutable. `show` verifies the signature, the registered key, and that the record belongs to that changeset.

Provenance is an **audit trail, not proof**: an agent can lie about its model or rationale. Correctness
comes from validation by a trusted issuer and from approvals. Treat `--input` and `--rationale` as
untrusted text.

## 6. Land it: what policy decides

An agent cannot run the integration step itself unless `agent_autonomy` allows it ([policy](policy.md)).
Typically a human or a service does:

```bash
ag integrate <proposal> [--fencing-token N]
# exit 0  integrated
# exit 3  blocked: missing validation / approval / provenance / lease token → provide it, run again
# exit 4  refused: conflict, denied by policy, or invalid
```

Re-running resumes the *same* operation. With `--json` the result has stable `status` and `blockers[].code`
values (`missing_validation`, `insufficient_approvals`, `changes_requested`, `missing_provenance`, `lease`,
`actor_not_allowed`, …) so an agent can decide what to do next without parsing prose.

### Autonomy envelope

```json
{ "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true }
```

With this, an agent-authored change that touches only `docs/**`, at most 5 files, no protected path, with
signed provenance and a trusted validation, lands **without a human**. Anything outside the envelope needs
human approvals (`approvals.min_for_agents`, default 1; agents do not count as approvers unless the policy
lists them).

## 7. A minimal agent loop

```bash
#!/usr/bin/env bash
set -euo pipefail
RUN=$(ag --json agent start my-agent | python3 -c 'import sys,json; print(json.load(sys.stdin)["identity"])')
export AG_IDENTITY=$RUN

ag lease acquire 'src/**' --exclusive --ttl 20m >/dev/null
TOKEN=$(ag --json lease list | python3 -c 'import sys,json; print(json.load(sys.stdin)[0]["token"])')

# … let the agent edit the workspace directory …

ag snapshot -m "$MESSAGE" --workspace work
PROP=$(ag --json propose @work -t "$TITLE" | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
ag provenance attach @work --agent my-agent --model "$MODEL"

set +e; ag integrate "$PROP" --fencing-token "$TOKEN"; rc=$?; set -e
case $rc in
  0) echo landed ;;
  3) echo "waiting on a human or CI; the operation resumes when I run the same command again" ;;
  4) ag preview "$PROP" && ag sync --workspace work ;;   # conflict: re-sync, resolve, snapshot, update the proposal
esac
ag agent finish "$(echo "$RUN" | sed 's/.*#//')"
```

## 8. Against a server: credentials, delegation and operators

Everything above also works over the network ([AGWP](../protocol/agwp-v1.md), `ag-server`). Three things are
specific to a server; the design and its limits are in [ADR-0010](../adr/0010-run-bound-credentials.md).

### A credential bound to a run

A credential minted for `agent:refactor-bot` is, by default, bound to the **name**: whoever holds it can act as
any run of the agent (`ag-run` header, heartbeat, complete, finish its tasks, release its leases). Two sessions of
one agent running side by side can therefore act for each other, and a token that leaked from a finished run
keeps working until it expires.

Bind the credential to **one run** instead: name the run in the scope, and the repository that holds it (exactly
one, no wildcards).

```json
POST /v1/credentials
{ "subject": "agent:refactor-bot",
  "scope": { "repos": ["acme/web"], "run_id": "run_…", "operations": ["objects.write", "proposal.create"] },
  "ttl_secs": 3600 }
```

* The run must exist, be `running` and belong to the subject; the minter must itself be allowed to run agents in
  that repository. The credential is capped like any agent credential, by its minter's expiry too.
* It **acts only as that run** - with or without an `ag-run` header - and naming another run is
  `permission_denied`. It cannot start runs, touch the workspaces, proposals and leases of another run, finish a
  task the run did not claim, submit provenance recorded for another run, or list and revoke credentials of other
  runs. It can mint credentials only for its own run, never wider than itself.
* It **dies with the run**: once the run is completed, failed or abandoned, every call answers `unauthenticated`
  ("the run this credential is bound to is no longer running"), an open event stream ends, and the credential
  shows as revoked in the listing. The check happens on every request, so a lost revocation cannot revive it.
* A client verifies the answer: a server without the feature (`auth.run_credentials` is missing from
  `GET /v1/capabilities`) would mint an ordinary credential and say nothing, so the orchestrator and `ag-mcp` refuse
  a credential whose scope does not echo the run.

The Go orchestrator mints a run-bound credential for every session it starts. `ag-mcp` ([MCP guide](mcp.md)) does the same for every run
it starts (`--run-credentials auto|required|off`, default `auto`: bound when the server supports it and the credential
is an agent's, otherwise carry on with the agent's own credential and say so in the result). Name-bound credentials
keep working; nothing forces a client to bind.

### The `delegator` role: minting credentials without being an administrator

`delegator` holds a writer's capabilities plus `credentials.delegate` (mint, list and revoke credentials for
**`agent:*` subjects only**) and `agent.operate` (below). It is what a service that starts agents - the
orchestrator - needs instead of `admin`:

```bash
ag-server admin grant --subject service:orchestrator --repo 'acme/*' --role delegator
```

A delegator **cannot escalate**: it cannot mint a credential for a person, a service, an administrator or a system
identity, and what it mints for an agent is always explicitly scoped to the capabilities the delegator itself holds on
every repository of the scope, less everything administrative (credentials, roles, policy, repository management,
GC/fsck, audit, stream CAS/force, and the operator powers). A request for more is `permission_denied`, an empty list
of operations means "everything it may pass on", and an agent that is more powerful than the delegator (an
administrator, say) still gets only what the delegator could do.

### Operators: finish tasks, end dead runs

A session that crashes leaves its run `running` and its task `in_progress`. With `agent.operate` (held by
`delegator` and `admin`):

* **Finish a task** (`POST …/tasks/{id}/finish`) that another identity owns. Anyone else may only finish their own
  task, and a run may only finish the task *it* claimed. The audit entry names the operator, says `operator: true` and
  records the owner it overrode.
* **Start, heartbeat or finish a run** for another agent (`POST …/runs`, `…/runs/{id}/heartbeat`, `…/runs/{id}/complete`). Without `agent.operate` the answer is `permission_denied`: an `agent:<name>` identity acts for itself only, and a person or a service with a plain `writer` role is not that agent. The audit entry of an operator finishing a run is the caller's, flagged `operator: true`, not the agent's. Finish a run the audit entry is the caller's, flagged `operator: true`, not the agent's.
* **Reap** (`POST …/runs/reap`, `ag-server admin reap-runs <repo>`): ends the `running` runs whose last heartbeat is
  older than `stale_after_ms` as `abandoned` (`run.abandon`), releases the leases of those runs and of any run that
  already ended (`lease.reap`), and revokes the credentials bound to them. `dry_run` reports without changing
  anything. A threshold below `[agents] min_stale_after_secs` (default 30 s) is refused, so a typo cannot end every
  live run.

```toml
[agents]
run_stale_after_secs = 900   # default threshold when the request names none
min_stale_after_secs = 30
reap_interval_secs = 0       # > 0: the server reaps every repository this often (audited as system:reaper)
```

A task is worked on by one run at a time: starting a second run for a task that already has a `running` run is
`failed_precondition`; it starts again once the first run has ended - finished, or reaped. Claiming a task that is
owned, blocked or finished is `failed_precondition` too.

The local `ag agent reap --timeout 10m` of section 1 ends silent runs and frees their leases in a repository you have on
disk (there are no credentials to revoke there). `ag agent reap --remote [NAME]` does what `POST …/runs/reap` does, for
an operator: it reaps the runs of the remote's repository, revokes their credentials, and prints what it did
(`--dry-run` only reports; without `--timeout` the server's `run_stale_after_secs` applies).

## 9. Embedding instead of shelling out

`ag-integrate` is a normal Rust library: `Repository::open(path)` gives you `snapshot`, `propose`,
`integrate`, `review`, `merge_preview`, `operation` and the rest with typed results (`Outcome`,
`IntegrationResult`). Open one `Repository` per thread (`repo.reopen()`); processes and threads coordinate
through SQLite. Over the network, use [`ag-server`](../operations/deployment.md) and its [clients](sdks.md) instead.

## 10. Discussing a remote proposal (development)

Use `ag remote review PROPOSAL --comment -m "Context for the reviewer"` to discuss a
proposal, including after it lands. On servers advertising `proposals.comments`,
writers can comment without approval rights. The CLI binds each comment to the head
it fetches immediately before submission and retries transport failures with the
same idempotency key. A concurrent head change rejects submission; inspect the new
head before running the command again. Comments preserve existing review decisions.
Older servers use the legacy review endpoint and its permissions.

## 11. Continuing after a failed run (development)

Open the run page to inspect its summary, saved output and any change requests.
A failed run may belong to an active task whose executor can still retry; use
**Review source task** before starting separate work.

For a finished or cancelled source task, **Create follow-up task…** opens an
editable task form with the original scope and description, source task/run links
and the failed run's summary. Review the failure and refine the expected result.
Choose the agent and prerequisites explicitly. Cancel leaves the repository
unchanged; **Create task** creates independent work and preserves the source task
and its run history. Configured executors may pick up the new task once created.
The source links are description text, not a structured retry relationship.

## 12. Discussing a specific changed line (development)

On a server advertising `proposals.threads`, select a line number in **Files
changed** to start a line discussion. Enter the question, then **Start discussion**.
The selected line and draft survive reloads. Replies stay with that discussion;
its author, proposal author, reviewers and integrators can resolve or reopen it.
A new reply invalidates a stale resolution attempt. The server verifies the line
and retains its exact source revision after the proposal changes.

Threads on a former head show **Earlier head**. They are not silently moved to a
new line, and resolving them does not grant approval. Use **Use general discussion
instead** to quote the line in the ordinary comment composer. Servers with only
`proposals.comments` keep this quote-only behavior. See
[the schema 6 upgrade and rollback guidance](../adr/0025-line-review-threads.md).

## 13. Line feedback through the CLI (development)

`ag remote discussion list PROPOSAL` returns line threads and their versions.
`ag remote discussion replies PROPOSAL THREAD` reads replies. Both return a
continuation token when more data exists; repeat with `--page-token TOKEN`.
Use `--json` for structured results. Capture the thread version before reading
its replies; if resolution is stale, read both again before retrying.

```sh
ag remote discussion start PROPOSAL --head INSPECTED_HEAD --path src/parser.rs --line 12 -m "Explain the fallback"
ag remote discussion reply PROPOSAL THREAD -m "The fallback preserves the previous behavior."
ag remote discussion resolve PROPOSAL THREAD --expected-version 2
ag remote discussion resolve PROPOSAL THREAD --expected-version 3 --reopen
```

Old-side anchors also require `--side old --base BASE_CHANGESET`. A changed head
rejects thread creation; new replies invalidate the version used for resolution.
Network retries reuse idempotency keys. No command in this group grants approval
or integrates code. The matching [MCP tools](mcp.md#line-feedback-from-people-and-agents-development)
let an executor read and respond to the same feedback.

## 14. Inspect execution history and output from the CLI (unreleased)

Find a run on the configured server, then inspect the same captured text shown on its Web page:

```sh
ag agent runs --remote
ag agent runs --remote --task TASK --active
ag agent runs --remote -n 0
ag agent output RUN --remote
ag agent output RUN --remote --stream stderr
ag --json agent output RUN --remote
```

The default run listing keeps the newest 50 records. `-n 0` follows all server pages;
`--task` filters before paging. Omit `--remote` when working directly in the local repository.
Readers can inspect output; deleting server output requires `agent.operate`:

```sh
ag agent output RUN --remote --delete
```

Deletion removes captured text and prevents later uploads for that run. The run, its outcome
and audit history remain. JSON reports `available: false` when nothing was captured. Selecting
a stream with no saved output fails instead of putting an explanatory message into a log pipe.
Text mode escapes terminal control characters; JSON retains the exact stored text. `--json`
and `--stream` are alternative output modes. Capture still requires operator opt-in, stores
bounded final tails, and is not live log streaming.
