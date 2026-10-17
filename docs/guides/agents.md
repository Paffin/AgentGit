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

## 8. Embedding instead of shelling out

`ag-integrate` is a normal Rust library: `Repository::open(path)` gives you `snapshot`, `propose`,
`integrate`, `review`, `merge_preview`, `operation` and the rest with typed results (`Outcome`,
`IntegrationResult`). Open one `Repository` per thread (`repo.reopen()`); processes and threads coordinate
through SQLite. A network API is on the [roadmap](../../ROADMAP.md).
