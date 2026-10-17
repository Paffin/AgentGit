-- AgentGit metadata schema, version 1 (PostgreSQL).
-- The same logical schema as migrations/sqlite/0001_init.sql.
--
-- * All timestamps are Unix milliseconds. All identifiers are lowercase hex.
-- * Every integer is BIGINT (no INT4 anywhere) and every text column is
--   COLLATE "C", so comparisons and ORDER BY are byte-wise exactly like SQLite's
--   default BINARY collation (prefix range scans and pagination depend on it).
-- * Autoincrement ids are BIGSERIAL. Sequences are not transactional: a rolled
--   back transaction leaves a gap in seq/id, never a duplicate or a reordering,
--   because write transactions are serialized by an advisory lock.
-- * There are deliberately no foreign keys (see docs/architecture/storage.md).

CREATE TABLE meta (
    key   TEXT COLLATE "C" PRIMARY KEY,
    value TEXT COLLATE "C" NOT NULL
);

-- Index of published ("ready") changesets. A row exists only after the
-- changeset and every blob it references were verified durable.
CREATE TABLE changesets (
    id           TEXT COLLATE "C" PRIMARY KEY,
    generation   BIGINT NOT NULL,
    fp_depth     BIGINT NOT NULL,
    author       TEXT COLLATE "C" NOT NULL,
    ts           BIGINT NOT NULL,
    first_parent TEXT COLLATE "C",
    parent_count BIGINT NOT NULL,
    ready_at     BIGINT NOT NULL,
    summary      TEXT COLLATE "C" NOT NULL
);

CREATE TABLE changeset_parents (
    child  TEXT COLLATE "C" NOT NULL,
    ord    BIGINT NOT NULL,
    parent TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (child, ord)
);
CREATE INDEX changeset_parents_by_parent ON changeset_parents (parent);

-- Paths touched by each changeset (path index; rebuildable from objects).
CREATE TABLE changeset_paths (
    changeset TEXT COLLATE "C" NOT NULL,
    path      TEXT COLLATE "C" NOT NULL,
    op        TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (changeset, path)
);
CREATE INDEX changeset_paths_by_path ON changeset_paths (path);

-- Derived, disposable: tree manifests and the changeset -> tree mapping.
CREATE TABLE trees (
    tree_id TEXT COLLATE "C" PRIMARY KEY,
    data    BYTEA NOT NULL
);
CREATE TABLE changeset_trees (
    changeset TEXT COLLATE "C" PRIMARY KEY,
    tree_id   TEXT COLLATE "C" NOT NULL
);

CREATE TABLE streams (
    name       TEXT COLLATE "C" PRIMARY KEY,
    head       TEXT COLLATE "C",
    version    BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    created_by TEXT COLLATE "C" NOT NULL
);

CREATE TABLE reflog (
    seq          BIGSERIAL PRIMARY KEY,
    stream       TEXT COLLATE "C" NOT NULL,
    old_head     TEXT COLLATE "C",
    new_head     TEXT COLLATE "C",
    version      BIGINT NOT NULL,
    operation_id TEXT COLLATE "C",
    actor        TEXT COLLATE "C" NOT NULL,
    ts           BIGINT NOT NULL,
    reason       TEXT COLLATE "C" NOT NULL
);
CREATE INDEX reflog_by_stream ON reflog (stream, seq);

-- Publication intents: durable record of every mutating operation.
CREATE TABLE intents (
    operation_id    TEXT COLLATE "C" PRIMARY KEY,
    idempotency_key TEXT COLLATE "C" NOT NULL UNIQUE,
    request_hash    TEXT COLLATE "C" NOT NULL,
    kind            TEXT COLLATE "C" NOT NULL,
    state           TEXT COLLATE "C" NOT NULL,
    actor           TEXT COLLATE "C" NOT NULL,
    stream          TEXT COLLATE "C",
    payload         TEXT COLLATE "C" NOT NULL,
    result          TEXT COLLATE "C",
    error           TEXT COLLATE "C",
    created_at      BIGINT NOT NULL,
    updated_at      BIGINT NOT NULL
);
CREATE INDEX intents_by_state ON intents (state, updated_at);

-- Objects staged by an in-flight publication; protects them from GC.
CREATE TABLE intent_objects (
    operation_id TEXT COLLATE "C" NOT NULL,
    object_id    TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (operation_id, object_id)
);

-- Transactional outbox: events committed atomically with state changes.
CREATE TABLE outbox (
    id            BIGSERIAL PRIMARY KEY,
    topic         TEXT COLLATE "C" NOT NULL,
    payload       TEXT COLLATE "C" NOT NULL,
    created_at    BIGINT NOT NULL,
    dispatched_at BIGINT,
    attempts      BIGINT NOT NULL DEFAULT 0,
    last_error    TEXT COLLATE "C"
);
CREATE INDEX outbox_pending ON outbox (id) WHERE dispatched_at IS NULL;

-- Hash-chained audit log (tamper-evident).
CREATE TABLE audit (
    seq       BIGSERIAL PRIMARY KEY,
    ts        BIGINT NOT NULL,
    actor     TEXT COLLATE "C" NOT NULL,
    action    TEXT COLLATE "C" NOT NULL,
    target    TEXT COLLATE "C" NOT NULL,
    details   TEXT COLLATE "C" NOT NULL,
    prev_hash TEXT COLLATE "C" NOT NULL,
    hash      TEXT COLLATE "C" NOT NULL
);

-- Append-only policy versions per stream.
CREATE TABLE policies (
    stream     TEXT COLLATE "C" NOT NULL,
    version    BIGINT NOT NULL,
    doc        TEXT COLLATE "C" NOT NULL,
    doc_hash   TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    created_by TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (stream, version)
);

CREATE TABLE proposals (
    id            TEXT COLLATE "C" PRIMARY KEY,
    stream        TEXT COLLATE "C" NOT NULL,
    head          TEXT COLLATE "C" NOT NULL,
    title         TEXT COLLATE "C" NOT NULL,
    description   TEXT COLLATE "C" NOT NULL,
    author        TEXT COLLATE "C" NOT NULL,
    state         TEXT COLLATE "C" NOT NULL,
    task_id       TEXT COLLATE "C",
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL,
    integrated_as TEXT COLLATE "C"
);
CREATE INDEX proposals_by_state ON proposals (state, stream);

CREATE TABLE reviews (
    id       BIGSERIAL PRIMARY KEY,
    proposal TEXT COLLATE "C" NOT NULL,
    reviewer TEXT COLLATE "C" NOT NULL,
    decision TEXT COLLATE "C" NOT NULL,
    body     TEXT COLLATE "C" NOT NULL,
    head     TEXT COLLATE "C" NOT NULL,
    ts       BIGINT NOT NULL
);
CREATE INDEX reviews_by_proposal ON reviews (proposal);

-- Signed statements from trusted execution planes, bound to an exact tree.
CREATE TABLE attestations (
    id                 TEXT COLLATE "C" PRIMARY KEY,
    tree_id            TEXT COLLATE "C" NOT NULL,
    check_name         TEXT COLLATE "C" NOT NULL,
    result             TEXT COLLATE "C" NOT NULL,
    policy_version     BIGINT NOT NULL,
    environment_digest TEXT COLLATE "C" NOT NULL,
    issuer             TEXT COLLATE "C" NOT NULL,
    issued_at          BIGINT NOT NULL,
    public_key         TEXT COLLATE "C" NOT NULL,
    signature          TEXT COLLATE "C" NOT NULL,
    payload            TEXT COLLATE "C" NOT NULL
);
CREATE INDEX attestations_by_tree ON attestations (tree_id);

-- Retention roots for garbage collection.
CREATE TABLE pins (
    name       TEXT COLLATE "C" PRIMARY KEY,
    changeset  TEXT COLLATE "C" NOT NULL,
    reason     TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE workspaces (
    name       TEXT COLLATE "C" PRIMARY KEY,
    stream     TEXT COLLATE "C" NOT NULL,
    base       TEXT COLLATE "C",
    head       TEXT COLLATE "C",
    owner      TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- Agent platform state.
CREATE TABLE agents (
    name       TEXT COLLATE "C" PRIMARY KEY,
    identity   TEXT COLLATE "C" NOT NULL UNIQUE,
    public_key TEXT COLLATE "C" NOT NULL,
    role       TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    disabled   BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE agent_runs (
    run_id         TEXT COLLATE "C" PRIMARY KEY,
    agent          TEXT COLLATE "C" NOT NULL,
    task_id        TEXT COLLATE "C",
    started_at     BIGINT NOT NULL,
    finished_at    BIGINT,
    status         TEXT COLLATE "C" NOT NULL,
    last_heartbeat BIGINT NOT NULL,
    summary        TEXT COLLATE "C"
);

CREATE TABLE tasks (
    id          TEXT COLLATE "C" PRIMARY KEY,
    title       TEXT COLLATE "C" NOT NULL,
    description TEXT COLLATE "C" NOT NULL,
    owner       TEXT COLLATE "C",
    state       TEXT COLLATE "C" NOT NULL,
    scope       TEXT COLLATE "C" NOT NULL,
    created_by  TEXT COLLATE "C" NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL
);

CREATE TABLE task_deps (
    task       TEXT COLLATE "C" NOT NULL,
    depends_on TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (task, depends_on)
);

CREATE TABLE leases (
    id          TEXT COLLATE "C" PRIMARY KEY,
    scope       TEXT COLLATE "C" NOT NULL,
    holder      TEXT COLLATE "C" NOT NULL,
    exclusive   BIGINT NOT NULL,
    token       BIGINT NOT NULL,
    acquired_at BIGINT NOT NULL,
    expires_at  BIGINT NOT NULL,
    released_at BIGINT,
    task_id     TEXT COLLATE "C"
);
CREATE INDEX leases_by_scope ON leases (scope);

CREATE TABLE counters (
    name  TEXT COLLATE "C" PRIMARY KEY,
    value BIGINT NOT NULL
);

-- Agent provenance: signed claims stored apart from the immutable changeset.
CREATE TABLE provenance (
    id         BIGSERIAL PRIMARY KEY,
    changeset  TEXT COLLATE "C" NOT NULL,
    agent      TEXT COLLATE "C" NOT NULL,
    run_id     TEXT COLLATE "C",
    record     TEXT COLLATE "C" NOT NULL,
    public_key TEXT COLLATE "C" NOT NULL,
    signature  TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX provenance_by_changeset ON provenance (changeset);

-- Integration queue: ordered, per-stream.
CREATE TABLE queue (
    id           BIGSERIAL PRIMARY KEY,
    stream       TEXT COLLATE "C" NOT NULL,
    proposal     TEXT COLLATE "C" NOT NULL,
    state        TEXT COLLATE "C" NOT NULL,
    enqueued_by  TEXT COLLATE "C" NOT NULL,
    enqueued_at  BIGINT NOT NULL,
    processed_at BIGINT,
    outcome      TEXT COLLATE "C"
);
CREATE INDEX queue_by_state ON queue (stream, state, id);
