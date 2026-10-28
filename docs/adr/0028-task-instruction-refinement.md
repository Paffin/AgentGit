# ADR 0028: refine task instructions only before first execution

Status: accepted.

## Context

A queued task can contain an unclear expected result or an overly broad file scope. Recreating it
loses its identity, links and assignment. Editing instructions after a run has started would instead
make historical work appear to have used instructions it never received.

## Decision

Allow the creator or an operator with task-write access to replace the title, description and scope
while the task is open and has no run history. Any run, including a failed or abandoned one, freezes
these instructions. Use an independent follow-up task to change the intent after execution.

The shared core validates title and path patterns, checks creator/operator authority, checks open
state and run history, compares the complete expected content, and writes content plus audit/outbox
inside one metadata write transaction. Run creation uses the same write boundary, so it cannot race
past the eligibility check. SQLite and PostgreSQL use the existing task and run tables; no metadata
migration or VCS format change is needed.

`task.update` accepts required `content` and `expected` objects (title, description, scope), and an
optional idempotency key. Content equality is a current-state precondition, not a revision counter or
an ABA detector. It does not depend on timestamp resolution. Other fields are not editable: owner,
creation metadata, prerequisites and source task/run links remain intact.

The server checks task-write access and creator/operator identity. A transaction checks for an
already recorded key before evaluating content preconditions, then records the exact response with
the update. An identical concurrent retry replays that response; a reused key with different content
is refused. A no-op produces no extra update audit/outbox entry. Replay can return the original
response after later activity; callers should reload when they need current state.

## User experience and compatibility

The Web UI opens a form from a queued task. Saving is explicit; failures preserve the draft.
A stale-content response asks the user to inspect the current task before another save. Account,
repository and task changes discard the old editing context and ignore late responses.
Older servers do not advertise `tasks.edit`, so clients must not offer or attempt the operation.
The CLI and generated SDKs use the same contract. The orchestrator rereads the task after
claiming it, before acquiring leases or writing executor input; a pre-edit dispatch snapshot
must not determine the executed instructions. There is no claim that editing pauses an executor:
if the executor starts first, the edit is rejected and a follow-up is required.
