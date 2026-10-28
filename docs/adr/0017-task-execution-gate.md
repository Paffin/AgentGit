# ADR 0017: operator control of new task runs

Status: implemented; qualification in progress.

## Decision

Store a repository-wide task scheduling decision in repository metadata: paused,
revision, reason, operator identity and update time. Readers can inspect it; changing
it requires `agent.operate` (delegator or administrator) and the expected revision.
The decision, audit entry and outbox event commit atomically.

The agent engine checks the decision inside the transaction that claims the task and
creates its run. A pause therefore cannot be bypassed by an old scheduler, another
replica, or a direct task-bound `StartRun` request. A run that committed before the
pause keeps running. Pausing is not process cancellation. Untasked/manual sessions
are outside this task-launch gate; the control is deliberately labelled accordingly.

The orchestrator reads the decision before scheduling, exposes the pause in status,
and defers ready tasks with `operator_pause`. It continues to reconcile existing work
and cancel sessions whose tasks were ended. Failure to read an implemented gate stops
new scheduling for that pass. A 404 from an older server is treated as lack of the
additive feature; its behavior remains unchanged.

The Web UI shows the actual stored decision, its reason and author. Resume explicitly
makes all ready tasks eligible for configured executors. The page does not claim that
an executor is installed or connected merely because task launches are allowed.

## Compatibility and recovery

Metadata schema 5 initializes the decision. Earlier binaries reject schema 5, instead
of silently ignoring a persisted pause. Back up before upgrading; downgrade requires
the pre-upgrade database. Both SQLite backups and PostgreSQL dumps include the gate.
Protocol additions are optional and advertised as `task.scheduling`; older Web clients
continue to work but cannot change the gate.

## Limits

This is operator scheduling control, not a browser-based executor installer. Configuration
of executable commands, model credentials and the orchestrator process is still an
infrastructure step. Per-task executor selection and browser-managed runner setup remain
open. No external coding-model run is claimed by the command-executor checks.
