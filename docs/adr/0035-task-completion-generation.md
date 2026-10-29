# ADR 0035: fence automatic task completion by execution generation

Status: accepted.

## Context

An orchestrator finishes tasks with operator credentials after its run credential stops
working. A delayed result from an earlier attempt must not finish work owned by a newer
attempt, even if both attempts use the same agent and retry limit.

## Decision

Add optional `expected_attempts_started` to `FinishTaskRequest` (protobuf field 6). The core
compares it to the durable count of admitted task runs in the same write transaction as
state and audit. A newer admitted run changes that count and stale completion is rejected
with HTTP 412 / failed_precondition, without changing the task or adding a finish audit.
The count includes failed and abandoned runs and is never reset by retry-policy edits.

Automatic session completion, cancellation, history recovery and proposal-result completion
supply the observed count. Cancellation also supplies the expected retry limit (ADR0034).
On a rejected transition, the scheduler discards local assumptions and reloads the task and
its history. Explicit manual completion without preconditions keeps its existing behavior.
Servers advertise `tasks.finish_preconditions`; the orchestrator verifies it before automatic completion and leaves the task unchanged on an older server. No storage migration or canonical-format change is required. Upgrade all replicas and
orchestrators together; this extends the unreleased task execution contract.
