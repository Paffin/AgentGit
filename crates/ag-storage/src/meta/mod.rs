//! The strongly consistent metadata store (SQLite, WAL, `synchronous=FULL`).
//!
//! Access is through two entry points that hand a [`Tx`] to a closure:
//!
//! * [`Meta::read`] runs the closure in a read transaction (a consistent snapshot);
//! * [`Meta::write`] runs it in a `BEGIN IMMEDIATE` transaction that commits
//!   if the closure returns `Ok` and rolls back otherwise.
//!
//! All domain operations are methods on [`Tx`], so callers compose several of
//! them into one atomic state change — for example the stream CAS, the
//! reflog entry, the integration record, the audit event and the outbox
//! event are written by a single `write` call.
//!
//! SQLite serializes writers, across threads *and* processes, which gives the
//! "no global repository lock, contention only on the stream being updated"
//! behaviour at the granularity the CAS compares: each write transaction is
//! short and holds the database write lock only for its own duration.

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

pub use agents::{AgentRow, LeaseRow, ProvenanceRow, QueueRow, RunRow, TaskRow};
pub use changesets::ChangesetInfo;
pub use intents::{IntentRow, IntentState, NewIntent};
pub use outbox::{AuditEntry, AuditReport, OutboxRow};
pub use records::{AttestationRow, NewProposal, PinRow, PolicyRow, ProposalRow, ReviewRow, WorkspaceRow};
pub use streams::{ReflogEntry, StreamRow, StreamState};

/// Schema version understood by this build.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = include_str!("schema.sql");

/// Errors from the metadata store.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    /// SQLite reported an error.
    #[error("metadata database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
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

pub(crate) fn parse<T>(s: &str, what: &str) -> Result<T, MetaError>
where
    T: FromStr,
    T::Err: Display,
{
    s.parse().map_err(|e| MetaError::Corrupt(format!("bad {what} '{s}': {e}")))
}

pub(crate) fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

pub(crate) fn stream_name(s: &str) -> Result<StreamName, MetaError> {
    parse(s, "stream name")
}

/// Handle to the metadata database. Cheap to share behind an `Arc`.
pub struct Meta {
    conn: Mutex<Connection>,
    clock: Arc<dyn Clock>,
    limits: Limits,
}

impl Meta {
    /// Opens (creating and migrating if necessary) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, MetaError> {
        Self::open_with(path, Arc::new(SystemClock), Limits::default())
    }

    /// Opens with an explicit clock and limits.
    pub fn open_with(path: &Path, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, MetaError> {
        let conn = Connection::open(path)?;
        Self::init(conn, clock, limits)
    }

    /// Opens a private in-memory database (tests).
    pub fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Self, MetaError> {
        Self::init(Connection::open_in_memory()?, clock, Limits::default())
    }

    fn init(conn: Connection, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, MetaError> {
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "trusted_schema", "OFF")?;
        let mut meta = Self { conn: Mutex::new(conn), clock, limits };
        meta.migrate()?;
        Ok(meta)
    }

    fn migrate(&mut self) -> Result<(), MetaError> {
        let now = self.clock.now();
        let conn = self.conn.get_mut().unwrap_or_else(|e| e.into_inner());
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(MetaError::UnsupportedSchema { found: version, supported: SCHEMA_VERSION });
        }
        if version < SCHEMA_VERSION {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Re-check under the write lock: another process may have migrated first.
            let current: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if current == 0 {
                tx.execute_batch(SCHEMA_V1)?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('created_at', ?1), ('schema', ?2)",
                    rusqlite::params![now.as_millis().to_string(), SCHEMA_VERSION.to_string()],
                )?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// The clock used for timestamps.
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// The limits applied when decoding stored data.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` in a read transaction (consistent snapshot).
    pub fn read<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        let mut guard = self.lock();
        let tx = guard.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let t = Tx { conn: &tx, now: self.clock.now(), limits: &self.limits };
        f(&t)
    }

    /// Runs `f` in a write transaction; commits on `Ok`, rolls back on `Err`.
    pub fn write<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        let mut guard = self.lock();
        let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = {
            let t = Tx { conn: &tx, now: self.clock.now(), limits: &self.limits };
            f(&t)?
        };
        tx.commit()?;
        Ok(out)
    }
}

/// A transaction scope handed to [`Meta::read`] / [`Meta::write`] closures.
pub struct Tx<'a> {
    pub(crate) conn: &'a Connection,
    now: Timestamp,
    pub(crate) limits: &'a Limits,
}

impl Tx<'_> {
    /// The time captured when the transaction started.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// Reads a key from the `meta` table.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>, MetaError> {
        use rusqlite::OptionalExtension;
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    /// Writes a key in the `meta` table.
    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

fn dag_err(e: MetaError) -> DagError {
    DagError::Source(ag_changeset::SourceError::Other(e.to_string()))
}

impl Graph for Meta {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        self.read(|tx| tx.parents(id)).map_err(dag_err)
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        self.read(|tx| tx.generation(id))
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
