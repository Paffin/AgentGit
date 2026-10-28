//! The strongly consistent metadata store: SQLite (WAL, `synchronous=FULL`) or
//! PostgreSQL, behind one API.
//!
//! Access is through two entry points that hand a [`Tx`] to a closure:
//!
//! * [`Meta::read`] runs the closure in a read transaction (a consistent snapshot);
//! * [`Meta::write`] runs it in a write transaction that commits if the closure
//!   returns `Ok` and rolls back otherwise.
//!
//! All domain operations are methods on [`Tx`], so callers compose several of
//! them into one atomic state change — for example the stream CAS, the
//! reflog entry, the integration record, the audit event and the outbox
//! event are written by a single `write` call.
//!
//! # Writers are serialized, readers are not
//!
//! Each write transaction is short and is the only writer of its repository
//! while it runs, which gives the "no global repository lock, contention only
//! on the stream being updated" behaviour at the granularity the CAS compares:
//!
//! | | SQLite | PostgreSQL |
//! |---|---|---|
//! | write | `BEGIN IMMEDIATE` (database write lock; across threads *and* processes) | `READ COMMITTED` + `pg_advisory_xact_lock` keyed by the schema name |
//! | read | deferred transaction (snapshot) | `REPEATABLE READ READ ONLY` |
//! | busy / lock wait | `busy_timeout` 30 s, then [`MetaError::Retriable`] | `lock_timeout` 30 s, then [`MetaError::Retriable`] |
//!
//! On SQLite one [`Meta`] handle owns a single connection behind a mutex, so
//! reads and writes through the *same handle* run one at a time; readers run
//! concurrently across handles (open a `Meta` per concurrent reader, as the
//! server's repository pool does) or processes. On PostgreSQL each transaction
//! takes its own pooled connection.
//!
//! The SQL itself is written once in the common subset (see [`db`]); the
//! backend only decides how a transaction is opened and closed.

pub mod db;
mod issues;
mod jobs;
mod migrate;
pub(crate) mod pg;
mod review_threads;

mod agents;
mod changesets;
mod derived;
mod intents;
mod outbox;
mod records;
mod streams;

use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ag_dag::{DagError, Graph};
use ag_types::{ChangesetId, Limits, StreamName, Timestamp};
use rusqlite::{Connection, TransactionBehavior};

use crate::clock::{Clock, SystemClock};
use db::{Conn, SqliteConn, params};

pub use agents::{AgentRow, LeaseRow, ProvenanceRow, QueueRow, RunRow, TaskRow};
pub use changesets::ChangesetInfo;
pub use db::{Dialect, Param, Row, Value};
pub use intents::{IntentRow, IntentState, NewIntent};
pub use issues::{IssueActivityRow, IssueFilter, IssueRow};
pub use jobs::{DEFAULT_JOB_ATTEMPTS, JobRow, JobState, NewJob};
pub use migrate::{Migration, SCHEMA_VERSION};
pub use outbox::{AuditEntry, AuditReport, OutboxRow};
pub use pg::{PgOptions, advisory_key, drop_schema, url_has_password, validate_schema_name};
pub use records::{
    AttestationRow, NewProposal, PinRow, PolicyRow, ProposalFilter, ProposalRow, ReviewRow, WorkspaceRow,
};
pub use review_threads::{ReviewThreadAnchorRow, ReviewThreadRow, ThreadCommentRow};
pub use streams::{ReflogEntry, StreamRow, StreamState};

/// Errors from the metadata store.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    /// SQLite reported an error.
    #[error("metadata database error: {0}")]
    Sqlite(rusqlite::Error),
    /// PostgreSQL reported an error (message includes the SQLSTATE).
    #[error("metadata database error: {0}")]
    Postgres(String),
    /// A transient failure: a serialization conflict, a deadlock, a lock or
    /// statement timeout, a busy SQLite database or an exhausted connection
    /// pool. Nothing was committed; running the same transaction again is safe.
    #[error("transient metadata failure (safe to retry): {0}")]
    Retriable(String),
    /// The connection to the database was lost. Nothing was reported as
    /// committed. If `during_commit` is true the outcome of the `COMMIT` is
    /// **unknown**: the transaction may or may not have been applied, and the
    /// caller must find out (for the engine: look the operation up by id)
    /// instead of blindly repeating it.
    #[error(
        "lost the connection to the metadata database{}: {message}",
        if *.during_commit { " while committing (the outcome is unknown)" } else { "" }
    )]
    ConnectionLost {
        /// The connection failed while the `COMMIT` was in flight.
        during_commit: bool,
        /// Driver message.
        message: String,
    },
    /// A stored JSON document could not be (de)serialized.
    #[error("metadata JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The entity already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// The stream moved since the caller read it.
    #[error("stream '{stream}' changed concurrently (expected version {expected}, found {actual})")]
    CasMismatch {
        /// Stream name.
        stream: String,
        /// Version the caller expected.
        expected: u64,
        /// Version currently stored.
        actual: u64,
    },
    /// An idempotency key was reused with a different request.
    #[error("idempotency key '{0}' was already used for a different request")]
    IdempotencyKeyReused(String),
    /// An intent state transition is not allowed.
    #[error("invalid intent transition {from} -> {to}")]
    InvalidTransition {
        /// Current state.
        from: String,
        /// Requested state.
        to: String,
    },
    /// A parent changeset has not been published yet.
    #[error("parent changeset {0} is not published")]
    ParentNotReady(ChangesetId),
    /// A caller-supplied value is invalid.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A count limit set by the caller (a repository quota) would be exceeded. Counted and refused inside the
    /// transaction that would have created the row, so concurrent creates cannot pass the limit between them.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// Stored data failed validation.
    #[error("metadata corruption: {0}")]
    Corrupt(String),
    /// The database was written by a newer version of AgentGit.
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema {
        /// Version found in the database.
        found: i64,
        /// Highest version this build supports.
        supported: i64,
    },
}

impl MetaError {
    /// True if running the same transaction again can succeed and cannot
    /// double-apply anything: [`MetaError::Retriable`], and a lost connection
    /// that was not in the middle of a `COMMIT`.
    pub fn is_retriable(&self) -> bool {
        matches!(self, MetaError::Retriable(_) | MetaError::ConnectionLost { during_commit: false, .. })
    }
}

impl From<rusqlite::Error> for MetaError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                MetaError::Retriable(format!("the database is busy: {e}"))
            }
            _ => MetaError::Sqlite(e),
        }
    }
}

pub(crate) fn parse<T>(s: &str, what: &str) -> Result<T, MetaError>
where
    T: FromStr,
    T::Err: Display,
{
    s.parse().map_err(|e| MetaError::Corrupt(format!("bad {what} '{s}': {e}")))
}

/// Escapes `\`, `%` and `_` for a `LIKE` pattern used with `ESCAPE '\'`.
pub(crate) fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub(crate) fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

pub(crate) fn stream_name(s: &str) -> Result<StreamName, MetaError> {
    parse(s, "stream name")
}

/// Which database engine backs a [`Meta`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaKind {
    /// An embedded SQLite database (a file, or memory in tests).
    Sqlite,
    /// A PostgreSQL schema.
    Postgres,
}

enum Backend {
    Sqlite(Mutex<Connection>),
    Postgres(Box<pg::PgBackend>),
}

/// Handle to the metadata database. Cheap to share behind an `Arc`.
pub struct Meta {
    backend: Backend,
    clock: Arc<dyn Clock>,
    limits: Limits,
    /// Write transactions committed through this handle (see [`Meta::committed_writes`]).
    writes: std::sync::atomic::AtomicU64,
}

fn migrate_sqlite(conn: &mut Connection, now: Timestamp) -> Result<(), MetaError> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(MetaError::UnsupportedSchema { found: version, supported: SCHEMA_VERSION });
    }
    if version < SCHEMA_VERSION {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-check under the write lock: another process may have migrated first.
        let current: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for m in migrate::SQLITE_MIGRATIONS.iter().filter(|m| m.version > current) {
            tx.execute_batch(m.sql).map_err(|e| m.failed(e.into()))?;
            let c = SqliteConn(&tx);
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('created_at', ?1) ON CONFLICT (key) DO NOTHING",
                &params![now.as_millis().to_string()],
            )?;
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('schema', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                &params![m.version.to_string()],
            )?;
            tx.pragma_update(None, "user_version", m.version)?;
        }
        tx.commit()?;
    }
    Ok(())
}

impl Meta {
    /// Opens (creating and migrating if necessary) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, MetaError> {
        Self::open_with(path, Arc::new(SystemClock), Limits::default())
    }

    /// Opens with an explicit clock and limits.
    pub fn open_with(path: &Path, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, MetaError> {
        let conn = Connection::open(path)?;
        Self::init_sqlite(conn, clock, limits)
    }

    /// Opens a private in-memory database (tests).
    pub fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Self, MetaError> {
        Self::init_sqlite(Connection::open_in_memory()?, clock, Limits::default())
    }

    /// Opens (creating and migrating if necessary) the repository stored in the
    /// PostgreSQL `schema` reachable through `url` (`postgres://…` or a
    /// `key=value` string; `sslmode` and `sslrootcert` are understood).
    ///
    /// One schema holds one repository: it is the unit of isolation, and its
    /// name keys the advisory lock that serializes the repository's writers.
    /// Several processes may open the same schema at once; the migration is
    /// serialized by an advisory lock too.
    pub fn open_postgres(url: &str, schema: &str, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, MetaError> {
        Self::open_postgres_with(url, schema, clock, limits, &PgOptions::default())
    }

    /// [`Meta::open_postgres`] with explicit pool and timeout settings.
    pub fn open_postgres_with(
        url: &str,
        schema: &str,
        clock: Arc<dyn Clock>,
        limits: Limits,
        opts: &PgOptions,
    ) -> Result<Self, MetaError> {
        let backend = pg::PgBackend::open(url, schema, opts, clock.now())?;
        Ok(Self { backend: Backend::Postgres(Box::new(backend)), clock, limits, writes: Default::default() })
    }

    fn init_sqlite(mut conn: Connection, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, MetaError> {
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "trusted_schema", "OFF")?;
        conn.set_prepared_statement_cache_capacity(128);
        migrate_sqlite(&mut conn, clock.now())?;
        Ok(Self { backend: Backend::Sqlite(Mutex::new(conn)), clock, limits, writes: Default::default() })
    }

    /// The database engine behind this handle.
    pub fn kind(&self) -> MetaKind {
        match self.backend {
            Backend::Sqlite(_) => MetaKind::Sqlite,
            Backend::Postgres(_) => MetaKind::Postgres,
        }
    }

    /// The PostgreSQL schema this handle works in (`None` for SQLite).
    pub fn schema_name(&self) -> Option<&str> {
        match &self.backend {
            Backend::Sqlite(_) => None,
            Backend::Postgres(p) => Some(p.schema()),
        }
    }

    /// The clock used for timestamps.
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// The limits applied when decoding stored data.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    fn sqlite(m: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes a consistent snapshot of the whole database to `dest` (which must
    /// not exist) using `VACUUM INTO`. Safe while other connections are writing.
    ///
    /// SQLite only: a PostgreSQL repository is backed up with `pg_dump` or
    /// continuous archiving (see `docs/operations/postgres-and-s3.md`), because
    /// a faithful copy needs the server's own tooling.
    pub fn backup_to(&self, dest: &Path) -> Result<(), MetaError> {
        let Backend::Sqlite(m) = &self.backend else {
            return Err(MetaError::Invalid(
                "this repository keeps its metadata in PostgreSQL; back it up with pg_dump (or PITR) and check the \
                 result with `ag verify` — see docs/operations/postgres-and-s3.md"
                    .into(),
            ));
        };
        if dest.exists() {
            return Err(MetaError::AlreadyExists(format!("backup target {}", dest.display())));
        }
        let guard = Self::sqlite(m);
        guard.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
        Ok(())
    }

    /// Runs `f` in a read transaction (consistent snapshot).
    pub fn read<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => {
                let mut guard = Self::sqlite(m);
                let tx = guard.transaction_with_behavior(TransactionBehavior::Deferred)?;
                let conn = SqliteConn(&tx);
                f(&Tx::new(&conn, self.clock.now(), &self.limits))
            }
            Backend::Postgres(p) => p.run(pg::TxKind::Read, &*self.clock, &self.limits, f),
        }
    }

    /// Like [`Meta::read`] for a closure that issues exactly **one** statement
    /// (a single lookup): on PostgreSQL it skips the explicit transaction, which
    /// saves two round trips per call. A second statement is an error.
    pub fn read_single<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        match &self.backend {
            Backend::Sqlite(_) => self.read(f),
            Backend::Postgres(p) => p.run(pg::TxKind::Single, &*self.clock, &self.limits, f),
        }
    }

    /// Runs `f` in a write transaction; commits on `Ok`, rolls back on `Err`.
    ///
    /// Write transactions of one repository never overlap, on any backend; the
    /// closure may therefore read a fact and act on it without it changing.
    pub fn write<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        let out = match &self.backend {
            Backend::Sqlite(m) => {
                let mut guard = Self::sqlite(m);
                let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let out = {
                    let conn = SqliteConn(&tx);
                    f(&Tx::new(&conn, self.clock.now(), &self.limits))?
                };
                tx.commit()?;
                out
            }
            Backend::Postgres(p) => p.run(pg::TxKind::Write, &*self.clock, &self.limits, f)?,
        };
        self.writes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(out)
    }

    /// How many write transactions this handle has committed since it was opened. Each is one durable commit
    /// (one fsync with SQLite's `synchronous=FULL`, one WAL flush on PostgreSQL), so the difference across an
    /// operation is the number of flushes it cost; the engine's tests pin it for the operations that were batched.
    pub fn committed_writes(&self) -> u64 {
        self.writes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of open PostgreSQL connections (0 for SQLite). Diagnostics and tests.
    #[doc(hidden)]
    pub fn open_connections(&self) -> usize {
        match &self.backend {
            Backend::Sqlite(_) => 0,
            Backend::Postgres(p) => p.open_connections(),
        }
    }
}

impl std::fmt::Debug for Meta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meta").field("kind", &self.kind()).field("schema", &self.schema_name()).finish()
    }
}

/// A transaction scope handed to [`Meta::read`] / [`Meta::write`] closures.
pub struct Tx<'a> {
    pub(crate) conn: &'a dyn Conn,
    now: Timestamp,
    pub(crate) limits: &'a Limits,
}

impl<'a> Tx<'a> {
    pub(crate) fn new(conn: &'a dyn Conn, now: Timestamp, limits: &'a Limits) -> Self {
        Self { conn, now, limits }
    }
}

impl Tx<'_> {
    /// The time captured when the transaction started.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// The SQL dialect of the backend (for the rare statement that differs).
    pub fn dialect(&self) -> Dialect {
        self.conn.dialect()
    }

    /// Reads a key from the `meta` table.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>, MetaError> {
        self.conn.query_opt("SELECT value FROM meta WHERE key = ?1", &params![key])?.map(|r| r.string(0)).transpose()
    }

    /// Writes a key in the `meta` table.
    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            &params![key, value],
        )?;
        Ok(())
    }

    /// Removes a key of the `meta` table (nothing if it is absent).
    pub fn meta_delete(&self, key: &str) -> Result<(), MetaError> {
        self.conn.execute("DELETE FROM meta WHERE key = ?1", &params![key])?;
        Ok(())
    }

    /// Up to `limit` keys that start with `prefix` and whose value sorts below `bound` as text (callers that
    /// store numbers pad them to a fixed width so that text order is numeric order).
    pub fn meta_keys_with_value_below(
        &self,
        prefix: &str,
        bound: &str,
        limit: usize,
    ) -> Result<Vec<String>, MetaError> {
        let like = format!("{}%", escape_like(prefix));
        self.conn
            .query(
                "SELECT key FROM meta WHERE key LIKE ?1 ESCAPE '\\' AND value < ?2 ORDER BY key LIMIT ?3",
                &params![like, bound, limit as i64],
            )?
            .iter()
            .map(|r| r.string(0))
            .collect()
    }

    /// For every key that starts with `prefix` and has no counterpart `companion_prefix` + the rest of the key, writes
    /// the counterpart with `value`; returns how many it wrote. One statement, so it is atomic and cheap to repeat.
    /// (The backfill of idempotency records that were written before they carried a time.)
    pub fn meta_fill_companions(&self, prefix: &str, companion_prefix: &str, value: &str) -> Result<u64, MetaError> {
        let like = format!("{}%", escape_like(prefix));
        let from = prefix.chars().count() as i64 + 1;
        self.conn.execute(
            "INSERT INTO meta (key, value)
             SELECT CAST(?2 AS TEXT) || substr(m.key, CAST(?4 AS INTEGER)), CAST(?3 AS TEXT) FROM meta m
             WHERE m.key LIKE ?1 ESCAPE '\\'
               AND NOT EXISTS (SELECT 1 FROM meta t WHERE t.key = CAST(?2 AS TEXT) || substr(m.key, CAST(?4 AS INTEGER)))
             ON CONFLICT (key) DO NOTHING",
            &params![like, companion_prefix, value, from],
        )
    }

    /// Executes arbitrary SQL (`?1` placeholders). For tests that simulate
    /// tampering or damage; never used by the engine.
    #[doc(hidden)]
    pub fn raw_execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        self.conn.execute(sql, params)
    }

    /// Runs arbitrary SQL and returns the rows. Tests only, like [`Tx::raw_execute`].
    #[doc(hidden)]
    pub fn raw_query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        self.conn.query(sql, params)
    }
}

fn dag_err(e: MetaError) -> DagError {
    DagError::Source(ag_changeset::SourceError::Other(e.to_string()))
}

impl Graph for Meta {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        self.read_single(|tx| tx.parents(id)).map_err(dag_err)
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        self.read_single(|tx| tx.generation(id))
            .map_err(dag_err)?
            .ok_or(DagError::Source(ag_changeset::SourceError::NotFound(*id)))
    }
}

impl Graph for Tx<'_> {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        Tx::parents(self, id).map_err(dag_err)
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        Tx::generation(self, id).map_err(dag_err)?.ok_or(DagError::Source(ag_changeset::SourceError::NotFound(*id)))
    }
}

#[cfg(test)]
mod tests;
