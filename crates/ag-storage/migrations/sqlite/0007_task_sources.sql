ALTER TABLE tasks ADD COLUMN source_task TEXT REFERENCES tasks(id);
ALTER TABLE tasks ADD COLUMN source_run TEXT REFERENCES agent_runs(run_id)
    CHECK (source_run IS NULL OR source_task IS NOT NULL);
CREATE INDEX tasks_source_task ON tasks(source_task);
CREATE INDEX tasks_source_run ON tasks(source_run);
