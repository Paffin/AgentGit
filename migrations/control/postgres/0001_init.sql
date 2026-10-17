-- ag-server control database, schema version 1 (PostgreSQL): who may do what to which repository.
-- The same logical schema as migrations/control/sqlite/0001_init.sql.
--
-- * All timestamps are Unix milliseconds; every integer is BIGINT; every text column is COLLATE "C", so
--   comparisons and ORDER BY are byte-wise exactly like SQLite's BINARY collation (repository listing
--   paginates by name).
-- * `meta` holds the schema version (key 'schema') and is maintained by the migration runner.
-- * There are deliberately no foreign keys: roles may name repositories that do not exist yet (patterns).

CREATE TABLE meta (
    key   TEXT COLLATE "C" PRIMARY KEY,
    value TEXT COLLATE "C" NOT NULL
);

CREATE TABLE credentials (
    id           TEXT COLLATE "C" PRIMARY KEY,
    secret_hash  TEXT COLLATE "C" NOT NULL,
    subject      TEXT COLLATE "C" NOT NULL,
    kind         TEXT COLLATE "C" NOT NULL,
    description  TEXT COLLATE "C" NOT NULL,
    scope_json   TEXT COLLATE "C" NOT NULL,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    revoked_at   BIGINT,
    last_used    BIGINT NOT NULL,
    created_by   TEXT COLLATE "C" NOT NULL
);
CREATE INDEX credentials_by_subject ON credentials (subject);

CREATE TABLE roles (
    subject    TEXT COLLATE "C" NOT NULL,
    repo       TEXT COLLATE "C" NOT NULL,
    role       TEXT COLLATE "C" NOT NULL,
    created_by TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (subject, repo, role)
);

CREATE TABLE repos (
    name           TEXT COLLATE "C" PRIMARY KEY,
    namespace      TEXT COLLATE "C" NOT NULL,
    default_stream TEXT COLLATE "C" NOT NULL,
    description    TEXT COLLATE "C" NOT NULL,
    created_at     BIGINT NOT NULL,
    created_by     TEXT COLLATE "C" NOT NULL,
    quota_json     TEXT COLLATE "C" NOT NULL,
    object_bytes   BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE idempotency (
    key          TEXT COLLATE "C" PRIMARY KEY,
    request_hash TEXT COLLATE "C" NOT NULL,
    result_json  TEXT COLLATE "C" NOT NULL,
    created_at   BIGINT NOT NULL
);

CREATE TABLE kv (
    key   TEXT COLLATE "C" PRIMARY KEY,
    value TEXT COLLATE "C" NOT NULL
);
