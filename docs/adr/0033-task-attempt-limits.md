# ADR 0033: durable task-local execution-attempt limits

Status: accepted.

## Context

Retry counts held only by an orchestrator are not a server-wide admission rule. Another client
or process can start another run, and a failed task may need more attempts without losing its
identity, dependencies or run history.

## Decision

Repository metadata schema 8 adds `task_attempt_limits`, keyed by task ID, and an index on
run history by task and creation order. A missing row or zero limit retains legacy server
behavior. An explicit limit is 1..10000 total task-bound runs, including active, failed,
abandoned and completed runs. Transport retries replaying an accepted start do not create
another run. Setup failures after run creation count; failures before admission do not.

The count is derived from immutable run history, never reset by process restart or an
operator's limit change. Admission checks it in the metadata transaction that claims the task
and starts the run. A failed transaction consumes no attempt. Increasing a limit neither
reopens finished tasks nor rewrites instructions or previous execution records.

Task creation accepts `max_attempts`; `task.set_attempt_limit` compares `expected_max_attempts`
and writes the limit, audit event and keyed response atomically. The creator may adjust it
before first execution. Afterwards an operator with `agent.operate` is required. Finished
or cancelled task policy stays immutable. Running executions are not cancelled by lowering
the limit; the next admission observes it.

The Task wire shape adds `max_attempts` and `attempts_started`. `ready` includes remaining
attempt availability. `tasks.attempt_limits` advertises support; older servers must not be
assumed to enforce an unknown field. The new update endpoint cannot silently succeed there.
Native CLI supports creation, inspection and optimistic updates locally and through a remote;
both SDKs expose the same operation.

## Orchestrator behavior

An explicit task limit takes precedence over the repository's legacy productive-attempt
setting. All admitted runs count, including temporary executor failures. Failed limited tasks
stay in progress: the scheduler reports `attempt_limit` rather than cancelling them when the
limit is exhausted. An operator can extend the limit and scheduling resumes with the same task
and history. Backoff is recovered from run history after restart. Tasks with no explicit limit
retain the existing repository retry behavior, including cancellation at that configured budget.

## Compatibility

Migration is additive and does not rewrite existing tasks or runs. Old binaries refuse schema 8
rather than ignoring the admission rule. Back up before upgrading and restore the pre-upgrade
backup to return to a schema-7 build. Existing creation-idempotency fingerprints remain unchanged
when `max_attempts` is zero; explicit limits extend the fingerprint.
