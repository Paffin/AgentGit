# ADR 0036: opt-in live execution output snapshots

Status: accepted.

## Context

Final diagnostic tails are useful after failure but do not show whether an executor is making
progress. Streaming must remain bounded, preserve the operator's explicit capture choice,
and never influence whether a change is correct or may be integrated.

## Decision

Keep `publish_output` false by default. With capture enabled, `output_interval` can opt into
periodic snapshots (1s–1m); zero preserves final-only capture. The Web setup wizard explicitly
offers live output and selects 2s. Each snapshot replaces the existing bounded output record,
retaining at most 32 KiB per stream. This is a snapshot stream, not an unbounded log archive.

A single output worker publishes changed snapshots with increasing producer sequence numbers.
It runs independently of heartbeats, is cancelled and joined before final publication, and
stops on deletion or a terminal permission/state refusal. Upload failure never changes the
execution result. Existing server sequence checks reject late snapshots; gaps are allowed.

Snapshot and audit share the existing metadata transaction. The audit-backed event stream
publishes `run.output`; idempotent replay adds no duplicate audit entry. Deletion publishes
`run.output.delete` without retaining output text, and the existing tombstone prevents
resurrection. Event payloads carry actor, target, sequence and byte counts, not log contents. Clients read the existing permission-checked output endpoint. Scheduler
reconciliation ignores diagnostic output events.

Known complete credentials and private signing-key encodings are redacted before capture.
Interim snapshots withhold a trailing prefix that could become a known secret in a later
write. Prefix matching is linear, including repetitive input. This does not claim detection
of arbitrary encoded or unknown secrets. Final capture retains its existing redaction behavior.
Output remains visible to repository readers and is never validation or approval evidence.

## Compatibility and qualification

No storage migration or new RPC is needed. Upgrade server and orchestrator together to get
live notifications. Old configurations retain final-only capture. A real command executor
qualifies partial-secret handling and output before completion. The browser setup scenario
launches the downloaded profile with the native orchestrator, observes a second output update
while the run is still active, then observes completion and revokes access. CI builds that
orchestrator once alongside the server and shares the binaries across browser jobs.
