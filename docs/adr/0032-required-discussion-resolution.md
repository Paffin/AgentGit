# ADR 0032: optional required resolution of line discussions

Status: accepted.

## Decision

Add `require_resolved_threads` to a stream policy, default false. A true value requires zero
unresolved line discussions on the proposal, including discussions anchored to earlier heads.
A head update cannot bypass feedback by making the anchor old. General comments and approval
records remain separate: they have no resolution state and are not counted.

The deterministic policy evaluator receives an unresolved-thread count as a fact and emits
`unresolved_threads` with a numeric count. It is a retryable blocked requirement, not a denial.
The count comes from the proposal's rows inside the same metadata transaction that evaluates
policy and commits the stream. Resolution/reopening uses that write boundary too. Reopening
before commit blocks; reopening after a completed integration does not reverse history.

Read-only preparation reports the same requirement. Completing an already-integrated proposal
also requires resolution even though it moves no stream; other existing no-op policy semantics
stay unchanged. Applying a code suggestion does not resolve its discussion automatically.

The Web rules form exposes the toggle only with `policy.resolved_threads` support. The client
explains the blocker and links to Reviews; the CLI's explanation supplies discussion commands.
The server remains authoritative for every client, including agents and local integration.

## Compatibility and rollout

No metadata migration or canonical-object change. False is omitted when serializing policy,
so old documents retain their default meaning. Old binaries reject unknown policy fields.
Upgrade every server/replica before enabling the rule; mixed-version operation with this rule
is unsupported. To roll back, use a supporting binary to save a policy without this field, or
restore a backup from before enabling it. Policy versions and historical records remain intact.

Presets do not enable the rule implicitly. Web preset changes preserve an existing explicit
choice. Thread author, proposal author, reviewer and integrator resolution permissions are
unchanged; the rule grants no new authority and cannot manufacture approvals.
