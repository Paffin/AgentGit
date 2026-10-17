-- ag-server control database, schema version 1 (SQLite): who may do what to which repository.
-- Unchanged from the single-file control database that shipped before the PostgreSQL control plane
-- (ADR-0007): an existing control.db is at version 1 already (PRAGMA user_version = 1).

CREATE TABLE credentials (
    id           TEXT PRIMARY KEY,
    secret_hash  TEXT NOT NULL,
    subject      TEXT NOT NULL,
    kind         TEXT NOT NULL,
    description  TEXT NOT NULL,
    scope_json   TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    revoked_at   INTEGER,
    last_used    INTEGER NOT NULL,
    created_by   TEXT NOT NULL
) STRICT;
CREATE INDEX credentials_by_subject ON credentials (subject);

CREATE TABLE roles (
    subject    TEXT NOT NULL,
    repo       TEXT NOT NULL,
    role       TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (subject, repo, role)
) STRICT, WITHOUT ROWID;

CREATE TABLE repos (
    name           TEXT PRIMARY KEY,
    namespace      TEXT NOT NULL,
    default_stream TEXT NOT NULL,
    description    TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    created_by     TEXT NOT NULL,
    quota_json     TEXT NOT NULL,
    object_bytes   INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE idempotency (
    key          TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    result_json  TEXT NOT NULL,
    created_at   INTEGER NOT NULL
) STRICT;

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
