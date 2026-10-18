# ADR-0010: Credentials bound to a run, a delegator role, and operator calls on runs

- Status: accepted
- Date: 2026-10-18
- Relates to: ADR-0001 ("a hijacked agent can still only propose"), ADR-0007 (control database, additive
  migrations), ADR-0009 (an identity's principal), [AGWP v1](../protocol/agwp-v1.md),
  [agents guide](../guides/agents.md), [orchestrator guide](../guides/orchestrator.md),
  [security model](../architecture/security.md), ROADMAP "server hardening that the integration work exposed"

## Context

An agent is `agent:<name>`; each execution of it is a **run**, `agent:<name>#<run_id>`. Until now the server bound a
credential to the *name*: whoever held a token of `agent:coder` could act as **any** run of `coder` (`ag-run` header,
`/runs/{id}/heartbeat`, `/complete`), finish the task another run had claimed, and release or extend that run's
leases. Two sessions of one agent running side by side - an orchestrator executor with `slots: 2`, two MCP sessions
of one agent platform - could therefore act for each other, and a token that leaked from a finished run still worked
until it expired. The Go orchestrator warned about it (`TestSameAgentRunsShareTheServersIdentity` pinned the
behaviour).

Two more gaps were the same problem seen from the operator's side:

* **The orchestrator needed `admin`.** Minting a credential for another identity required `credentials.mint`, which
  only administrators had. A service that exists to hand out agent credentials therefore held every administrative
  power (policy, roles, GC) and was kept small only by the scope of its own credential.
* **Nothing ended a dead run on the server.** A crashed session leaves a run `running` and its task `in_progress`; the
  orchestrator completed such runs and minted a one-minute credential *as the task's owner* to finish the task,
  because only an owner could.

## Decision

### 1. A credential can be bound to one run

`CredentialScope` gains `run_id`. A credential whose scope names a run (and exactly one literal repository in `repos`
- the run lives in that repository) is **run-bound**:

* *Minting.* The run must exist, be `running` and belong to the credential's subject (`agent:<name>`, never a
  subject that names a run). The minter must itself be allowed `agent.run` on the repository, so asking about a run is
  not a way to learn that it exists. The lifetime is capped as for any agent credential and by the minter's own expiry.
  A run-bound credential can mint further credentials only for **its own run** (`Scope::contains`), narrowed as usual.
* *Enforcement.* One check at the front door - `App::dispatch`, after authentication, before any operation - looks
  the run up: not running (finished, failed, abandoned, gone) means `unauthenticated`, exactly as for a revoked
  credential. Behind it the acting identity of every repository operation (`effective_actor`) is the bound run,
  with or without an `ag-run` header, and naming another run is `permission_denied`. The operations that name a run
  or something a run owns hold run-bound actors to **their** run: heartbeat and completion of a run, starting a
  run (a run does not start runs), the workspaces, proposals and leases a run created (`owns`: when both name a run it
  must be the same run), the task a run claimed, provenance (the record must be for that run), and the credentials
  of the run (a run-bound credential lists and revokes only those). Scope containment already confines it to one
  repository and cannot reach global operations.
* *End.* Finishing a run (`complete`, an operator's completion, a reap) revokes the credentials bound to it, so the
  listing shows them revoked and an open event stream ends (every poll of the log is an operation of its own and meets the same check). The use-time check is the
  guarantee; the revocation is the record. Expiry is the credential's ordinary expiry.
* *Storage.* The binding is part of the stored scope and is repeated in two indexed columns of `credentials`
  (control schema **3**, additive, both backends) so the credentials of a run are found with an index and so that a
  build that predates the binding refuses the database (newer-schema refusal) instead of reading a run-bound
  credential as an ordinary one. Backups carry the columns (`ControlDump`); older dumps restore with them empty.

Name-bound credentials keep working unchanged: they remain the agent itself, for every run. What changes is the
default of the clients we ship: the orchestrator mints a run-bound credential for every session, and `ag-mcp`
mints one for every run it starts (`--run-credentials auto|required|off`, default `auto`: bound when the server
supports it and the credential is an agent's, otherwise carry on and say so). A client checks the answer: a server
that predates the field would mint an ordinary credential and say nothing, so both clients refuse a credential whose
scope does not echo the run.

### 2. The `delegator` role

`delegator` holds a writer's capabilities and two new ones:

* `credentials.delegate` - mint, list and revoke credentials for **`agent:*` subjects only** (never a person, a
  service, an administrator or a system identity), and only for the repositories it holds the role on;
* `agent.operate` - the operator calls below.

*No escalation.* A delegated credential is always **explicitly scoped** to a set of capabilities that is the
*intersection*, over the repositories of the requested scope, of (a) what the delegator's own roles grant there, (b)
what its own credential's scope allows, minus (c) `NEVER_DELEGATED`: everything administrative (credentials, roles,
policy, repository management, GC/fsck, audit, stream CAS/force) and the operator capabilities themselves. An empty
`operations` list is replaced by that set; a request for anything outside it is `permission_denied`. So an agent whose
*own* bindings are broader (`agent:boss` is an administrator) still gets from a delegator only what the delegator
could do itself. A test pins that every capability only administrators have is on the list, so a new administrative
capability cannot silently become delegable.

The orchestrator is bound `delegator` and no longer needs `admin`; `OrchestratorOps` asks for
`credentials.delegate` and `agent.operate` instead of `credentials.mint` / `credentials.revoke`.

### 3. Operator calls on tasks and runs

* **Finish a task** (`task.finish`) as a non-owner needs `agent.operate`; the audit entry names the operator, says
  `operator: true` and records the owner it overrode. The owner-only rule stands for everyone else, and a *run* may
  only finish the task **it** claimed.
* **Finish a run** keeps its authorization (`agent.run`, and a run-bound caller only its own run), but the audit entry
  is the caller's, flagged `operator: true` when it is not the run's agent: the log no longer shows an agent doing what
  an operator did.
* **Reap** (`POST /v1/repos/{owner}/{name}/runs/reap`, `agent.operate`): ends the `running` runs whose last heartbeat is
  older than `stale_after_ms`, releases the leases of those runs *and of any run that has ended*, and revokes their
  credentials. Each run is audited as `run.abandon` and each lease as `lease.reap`, under the operator. `dry_run`
  reports without changing anything. The threshold defaults to `[agents] run_stale_after_secs` and cannot be set
  below `[agents] min_stale_after_secs`, so a typo cannot end every live run. The same logic runs offline
  (`ag-server admin reap-runs`) and, when `[agents] reap_interval_secs` is set, on a timer for every repository,
  audited as `system:reaper`. The run status the protocol documents is `abandoned` (the comment used to say `stale`).
* **One run per task.** Starting a second run for a task that already has a `running` run is refused
  (`failed_precondition`); otherwise two runs of the same agent could be on the same files, which the claim rule (same
  owner) allowed. The task starts again once the first run has ended - finished, or reaped.

Claim conflicts (`start_run` on a task that is owned, blocked or finished) are now `failed_precondition`, as the
orchestrator already assumed, not `invalid_argument`.

### 4. Two smaller changes in the same package

* `integration.prepare` reads the policy verdict, the attestations and the jobs that validate the candidate in **one**
  read transaction (`Repository::prepare_with`), so a validation that finishes at that moment is reported as running
  *or* as passed, never as "missing with nothing in flight". With `enqueue_validations` the whole answer is read again
  after the jobs were queued.
* Repository listings and `create` carry the real stream and changeset counts (`counts_error` says why when a
  repository cannot be read). A listing opens the repositories of its page; the page size bounds the price.

## Alternatives considered

* **Bind by a new credential kind (`run:`).** A new identity kind would touch every comparison that today understands
  `agent:name#run` (principals, approvals, ownership) and the format of stored identities. The scope already is where
  credentials are narrowed, and `Scope::contains` gives "a run-bound credential mints only for its own run" for free.
* **Check the run only where operations name it.** Easy to forget in the next operation; the single check in
  `dispatch` plus the acting identity makes the guarantee structural, and the per-operation rules above only decide
  what a *run* may touch.
* **A delegator that mints anything an agent's own roles grant.** Simpler, but it makes the delegator exactly as
  powerful as the most powerful agent it can name. Delegation that can only attenuate is the property that makes the
  role safe to give to a service.
* **Reaping only in the orchestrator.** It already completed orphaned runs, but deployments without an orchestrator
  (agents through MCP) had nobody to do it, and an operator needs the same call.
* **Making `writer` unable to act for other agents** (finish or heartbeat their runs, start runs for them). The right
  end state, but writers use it today (SDK examples, the Web UI's demo world) and the change is not needed for the
  properties above; it is listed under follow-ups.

## Consequences

* ✅ Concurrent runs of one agent cannot act for each other when their credentials are run-bound (the orchestrator's
  always are); a leaked session credential dies with the run; the orchestrator holds no administrator role.
* ✅ Operators can end dead runs, free their leases and finish stuck tasks through the API, with audit entries that
  name them.
* ⚠️ A name-bound credential is still the agent for every run: nothing forces a client to bind. `ag-mcp` in `off` mode
  and any hand-minted agent token behave as before. The protocol advertises `auth.run_credentials`,
  `auth.delegation` and `agents.operate` so clients can require them.
* ⚠️ A non-agent principal with `agent.run` (a writer) can still start, heartbeat and complete the runs of any
  agent. Those actions are now attributed (and flagged as operator actions when they are not the agent's own), but not
  prevented. Follow-up: require `agent.operate` for acting on another agent's runs.
* ⚠️ Lease ownership between *plain* actors stays by principal (documented in the agents guide): an agent acting
  without a run can release the leases of its runs, and the overlap rule between two runs of one agent is unchanged.
* ⚠️ Control schema 3 is additive, but a build that only knows schema 2 refuses the migrated database. A rolling
  upgrade across replicas starts the new build first; ADR-0007 describes the procedure.
* ⚠️ Not verified here: a multi-replica run of the new paths (the revocation and the use-time check use the control
  database and the repository, both shared; `crates/ag-server/tests/multi_replica.rs` is not on `main`), and a real
  agent platform driving `ag-mcp` with run credentials.
