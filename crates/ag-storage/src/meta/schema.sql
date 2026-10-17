-- AgentGit metadata schema, version 1.
-- All timestamps are Unix milliseconds. All identifiers are lowercase hex.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- Index of published ("ready") changesets. A row exists only after the
-- changeset and every blob it references were verified durable.
CREATE TABLE changesets (
    id           TEXT PRIMARY KEY,
    generation   INTEGER NOT NULL,
    fp_depth     INTEGER NOT NULL,
    author       TEXT NOT NULL,
    ts           INTEGER NOT NULL,
    first_parent TEXT,
    parent_count INTEGER NOT NULL,
    ready_at     INTEGER NOT NULL,
    summary      TEXT NOT NULL
) STRICT;

CREATE TABLE changeset_parents (
    child  TEXT NOT NULL,
    ord    INTEGER NOT NULL,
    parent TEXT NOT NULL,
    PRIMARY KEY (child, ord)
) STRICT, WITHOUT ROWID;
CREATE INDEX changeset_parents_by_parent ON changeset_parents (parent);

-- Paths touched by each changeset (path index; rebuildable from objects).
CREATE TABLE changeset_paths (
    changeset TEXT NOT NULL,
    path      TEXT NOT NULL,
    op        TEXT NOT NULL,
    PRIMARY KEY (changeset, path)
) STRICT, WITHOUT ROWID;
CREATE INDEX changeset_paths_by_path ON changeset_paths (path);

-- Derived, disposable: tree manifests and the changeset -> tree mapping.
CREATE TABLE trees (
    tree_id TEXT PRIMARY KEY,
    data    BLOB NOT NULL
) STRICT;
CREATE TABLE changeset_trees (
    changeset TEXT PRIMARY KEY,
    tree_id   TEXT NOT NULL
) STRICT;

CREATE TABLE streams (
    name       TEXT PRIMARY KEY,
    head       TEXT,
    version    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    created_by TEXT NOT NULL
) STRICT;

CREATE TABLE reflog (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    stream       TEXT NOT NULL,
    old_head     TEXT,
    new_head     TEXT,
    version      INTEGER NOT NULL,
    operation_id TEXT,
    actor        TEXT NOT NULL,
    ts           INTEGER NOT NULL,
    reason       TEXT NOT NULL
) STRICT;
CREATE INDEX reflog_by_stream ON reflog (stream, seq);

-- Publication intents: durable record of every mutating operation.
CREATE TABLE intents (
    operation_id    TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    request_hash    TEXT NOT NULL,
    kind            TEXT NOT NULL,
    state           TEXT NOT NULL,
    actor           TEXT NOT NULL,
    stream          TEXT,
    payload         TEXT NOT NULL,
    result          TEXT,
    error           TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
) STRICT;
CREATE INDEX intents_by_state ON intents (state, updated_at);

-- Objects staged by an in-flight publication; protects them from GC.
CREATE TABLE intent_objects (
    operation_id TEXT NOT NULL,
    object_id    TEXT NOT NULL,
    PRIMARY KEY (operation_id, object_id)
) STRICT, WITHOUT ROWID;

-- Transactional outbox: events committed atomically with state changes.
CREATE TABLE outbox (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    topic         TEXT NOT NULL,
    payload       TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    dispatched_at INTEGER,
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT
) STRICT;
CREATE INDEX outbox_pending ON outbox (id) WHERE dispatched_at IS NULL;

-- Hash-chained audit log (tamper-evident).
CREATE TABLE audit (
    seq       INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    actor     TEXT NOT NULL,
    action    TEXT NOT NULL,
    target    TEXT NOT NULL,
    details   TEXT NOT NULL,
    prev_hash TEXT NOT NULL,
    hash      TEXT NOT NULL
) STRICT;

-- Append-only policy versions per stream.
CREATE TABLE policies (
    stream     TEXT NOT NULL,
    version    INTEGER NOT NULL,
    doc        TEXT NOT NULL,
    doc_hash   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    PRIMARY KEY (stream, version)
) STRICT, WITHOUT ROWID;

CREATE TABLE proposals (
    id            TEXT PRIMARY KEY,
    stream        TEXT NOT NULL,
    head          TEXT NOT NULL,
    title         TEXT NOT NULL,
    description   TEXT NOT NULL,
    author        TEXT NOT NULL,
    state         TEXT NOT NULL,
    task_id       TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    integrated_as TEXT
) STRICT;
CREATE INDEX proposals_by_state ON proposals (state, stream);

CREATE TABLE reviews (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal TEXT NOT NULL,
    reviewer TEXT NOT NULL,
    decision TEXT NOT NULL,
    body     TEXT NOT NULL,
    head     TEXT NOT NULL,
    ts       INTEGER NOT NULL
) STRICT;
CREATE INDEX reviews_by_proposal ON reviews (proposal);

-- Signed statements from trusted execution planes, bound to an exact tree.
CREATE TABLE attestations (
    id                 TEXT PRIMARY KEY,
    tree_id            TEXT NOT NULL,
    check_name         TEXT NOT NULL,
    result             TEXT NOT NULL,
    policy_version     INTEGER NOT NULL,
    environment_digest TEXT NOT NULL,
    issuer             TEXT NOT NULL,
    issued_at          INTEGER NOT NULL,
    public_key         TEXT NOT NULL,
    signature          TEXT NOT NULL,
    payload            TEXT NOT NULL
) STRICT;
CREATE INDEX attestations_by_tree ON attestations (tree_id);

-- Retention roots for garbage collection.
CREATE TABLE pins (
    name       TEXT PRIMARY KEY,
    changeset  TEXT NOT NULL,
    reason     TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE workspaces (
    name       TEXT PRIMARY KEY,
    stream     TEXT NOT NULL,
    base       TEXT,
    head       TEXT,
    owner      TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- Agent platform state.
CREATE TABLE agents (
    name       TEXT PRIMARY KEY,
    identity   TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    role       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    disabled   INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE agent_runs (
    run_id         TEXT PRIMARY KEY,
    agent          TEXT NOT NULL,
    task_id        TEXT,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    status         TEXT NOT NULL,
    last_heartbeat INTEGER NOT NULL,
    summary        TEXT
) STRICT;

CREATE TABLE tasks (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    description TEXT NOT NULL,
    owner       TEXT,
    state       TEXT NOT NULL,
    scope       TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE task_deps (
    task       TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (task, depends_on)
) STRICT, WITHOUT ROWID;

CREATE TABLE leases (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,
    holder      TEXT NOT NULL,
    exclusive   INTEGER NOT NULL,
    token       INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    released_at INTEGER,
    task_id     TEXT
) STRICT;
CREATE INDEX leases_by_scope ON leases (scope);

CREATE TABLE counters (
    name  TEXT PRIMARY KEY,
    value INTEGER NOT NULL
) STRICT;

-- Agent provenance: signed claims stored apart from the immutable changeset.
CREATE TABLE provenance (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    changeset  TEXT NOT NULL,
    agent      TEXT NOT NULL,
    run_id     TEXT,
    record     TEXT NOT NULL,
    public_key TEXT NOT NULL,
    signature  TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX provenance_by_changeset ON provenance (changeset);

-- Integration queue: ordered, per-stream.
CREATE TABLE queue (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    stream       TEXT NOT NULL,
    proposal     TEXT NOT NULL,
    state        TEXT NOT NULL,
    enqueued_by  TEXT NOT NULL,
    enqueued_at  INTEGER NOT NULL,
    processed_at INTEGER,
    outcome      TEXT
) STRICT;
CREATE INDEX queue_by_state ON queue (stream, state, id);
