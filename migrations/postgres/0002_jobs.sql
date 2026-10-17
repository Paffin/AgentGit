-- Schema version 2: the asynchronous job queue ("PostgreSQL-backed queue" of
-- ADR-0001 section 10; here for both backends).
--
-- A job is claimed with a lease (claimed_by / claimed_until). A claim whose
-- lease expired is reaped by the next claimer: the job is queued again, or
-- failed once max_attempts is used up. dedupe_key makes enqueueing idempotent
-- (NULLs do not collide). Claiming uses FOR UPDATE SKIP LOCKED.

CREATE TABLE jobs (
    id            TEXT COLLATE "C" PRIMARY KEY,
    kind          TEXT COLLATE "C" NOT NULL,
    state         TEXT COLLATE "C" NOT NULL CHECK (state IN ('queued', 'claimed', 'succeeded', 'failed', 'cancelled')),
    payload       TEXT COLLATE "C" NOT NULL,
    result        TEXT COLLATE "C",
    error         TEXT COLLATE "C",
    dedupe_key    TEXT COLLATE "C" UNIQUE,
    attempts      BIGINT NOT NULL DEFAULT 0,
    max_attempts  BIGINT NOT NULL,
    run_after     BIGINT NOT NULL,
    claimed_by    TEXT COLLATE "C",
    claimed_until BIGINT,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL
);
CREATE INDEX jobs_by_state ON jobs (state, run_after);
CREATE INDEX jobs_by_lease ON jobs (claimed_until) WHERE state = 'claimed';
