-- Task-local retry limits. Absence means the legacy unlimited server behavior.
CREATE TABLE task_attempt_limits (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 0 AND 10000)
) STRICT;
CREATE INDEX agent_runs_by_task ON agent_runs(task_id, started_at DESC, run_id DESC);
