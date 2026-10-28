-- Older binaries must refuse this schema instead of ignoring an operator's pause.
INSERT INTO meta (key, value) VALUES ('task_scheduling', '{"paused":false,"version":0,"reason":"","updated_by":"","updated_at_ms":0}')
ON CONFLICT(key) DO NOTHING;
