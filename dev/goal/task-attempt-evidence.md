# Durable task execution limits — development evidence

The branch implements server admission against durable run history, optimistic limit updates,
and orchestrator recovery/continuation. It is not a published release.

- Red→green: a third run against a two-run limit was admitted before the core guard and rejected after it.
- Agent coordination suite: 27 passed in 0.19s after final refactor; rollback does not consume an attempt. Two focused admission/rollback tests also passed against required PostgreSQL in 0.52s.
- Real HTTP server: limit exhaustion, keyed replay, creator/operator permissions, stale limit rejection, extension and second run passed.
- Real server and shell executor: two failed runs, scheduler restart, preserved exhaustion, operator extension, successful third run on the same task.
- SQLite and required PostgreSQL migrations: five schema scenarios each passed, preserving schema-7 tasks and runs.
- Local/remote CLI: three focused scenarios passed; both JSON contract checks passed.
- TypeScript SDK: 12 tests; Python SDK: 13 tests, including all 120 operations.
- Web typecheck passed after adding the new required fields to the graph fixture.

Raw logs: `/workspace/agentgit-quality/task-attempt-*.log` in the implementation workspace.
Crapper and Mutator were run as static scans, not as measured coverage or mutation-kill evidence;
Dryer found no duplicate candidates in the touched core task/registry modules.

The browser shows exhausted limits; a browser policy editor remains open. The repository's
legacy retry policy is retained for tasks with max_attempts=0. Schema 8 requires a pre-upgrade backup.
