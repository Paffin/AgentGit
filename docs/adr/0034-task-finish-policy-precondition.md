# ADR 0034: task completion can compare its execution policy

Status: accepted.

## Context

An operator can install a task attempt limit while an executor is running. The orchestrator
may still hold the previous task snapshot and decide to cancel after its legacy retry budget.
An unguarded state update would cancel work the operator deliberately made resumable.
A separate read before cancellation would leave the same race before the write.

## Decision

`FinishTaskRequest` adds optional `expected_max_attempts` (field 5). If supplied, the core compares
it to the current limit inside the task-state write transaction. Mismatch returns
`failed_precondition` (HTTP 412), with no state update or finish audit event. Explicit zero
compares an unset limit; omission retains ordinary manual completion behavior.

The orchestrator supplies the observed limit when automatically cancelling after a failed
session or exhausting recovered history. A stale cancellation leaves the task in progress;
the next reconciliation uses its current policy. Successful completion and explicit manual
completion retain their existing semantics. No new persisted format or metadata migration.
The field belongs to the unreleased `tasks.attempt_limits` contract; upgrade all replicas
and the orchestrator together before relying on automatic cancellation protection.

## Evidence

A real server and shell executor reproduce the failure: changing a running task from an unset
limit to one attempt was followed by cancellation under the old policy. The regression keeps
that task resumable and completes the same task after extension. Core and HTTP checks cover
stale cancellation rejection and preservation of state and audit history.
