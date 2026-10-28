# ADR 0026: Per-agent admission of new runs

Status: implemented in development.

Operators need to take a registered agent out of rotation without deleting its
identity, key, history or currently running work. Reuse the existing `disabled`
flag rather than introduce a second scheduling state. No metadata migration or
VCS format change is required.

The local CLI and API share the same transactional state-change function.
The CLI `agent list/disable/enable --remote NAME` uses the same public gRPC API,
negotiates `agents.state` for changes and captures the observed state before
submitting a guarded, retry-safe mutation.
Starting a run reads the agent's state inside the run creation transaction, so a
start and a disable are serialized by repository metadata writes. Once disable
commits, later starts are refused. Runs already admitted remain valid; disabling
is not credential revocation or process termination. Task cancellation and run
reaping retain their separate meanings.

`PATCH /agents/{name}` and `AgentService.SetState` require `agent.operate` and the
existing observed `expected_disabled` value. This is a current-state comparison,
not a revision history check. An idempotency key records the response in the same
transaction as the state change, audit record and `agent.state` outbox event.
Retries replay that response; reusing a key for another actor or request fails.
Repeating a matching no-op state does not add another audit event.

The `agents.state` feature controls the browser actions. The confirmation names
the agent and explains that existing sessions continue. The UI captures the
observed state before confirmation, preserves errors, and discards late results
after repository/account changes. Older servers retain a read-only status column.
