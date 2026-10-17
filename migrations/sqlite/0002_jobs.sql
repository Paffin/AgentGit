-- Schema version 2: the asynchronous job queue ("PostgreSQL-backed queue" of
-- ADR-0001 section 10; here for both backends).
--
-- A job is claimed with a lease (claimed_by / claimed_until). A claim whose
-- lease expired is reaped by the next claimer: the job is queued again, or
-- failed once max_attempts is used up. dedupe_key makes enqueueing idempotent
-- (NULLs do not collide).

CREATE TABLE jobs (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('queued', 'claimed', 'succeeded', 'failed', 'cancelled')),
    payload       TEXT NOT NULL,
    result        TEXT,
    error         TEXT,
    dedupe_key    TEXT UNIQUE,
    attempts      INTEGER NOT NULL DEFAULT 0,
    max_attempts  INTEGER NOT NULL,
    run_after     INTEGER NOT NULL,
    claimed_by    TEXT,
    claimed_until INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;
CREATE INDEX jobs_by_state ON jobs (state, run_after);
CREATE INDEX jobs_by_lease ON jobs (claimed_until) WHERE state = 'claimed';
