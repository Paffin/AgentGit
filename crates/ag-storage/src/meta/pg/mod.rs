//! The PostgreSQL metadata backend.
//!
//! # Transaction semantics
//!
//! The engine relies on SQLite's `BEGIN IMMEDIATE`: one writer at a time per
//! repository, reading a state that nobody can change until it commits. On
//! PostgreSQL the same guarantee is built from two parts:
//!
//! * **Write transactions** run at `READ COMMITTED` and take
//!   `pg_advisory_xact_lock(key)` as their first statement, where `key` is
//!   derived from the repository's schema name. Writers of one repository are
//!   therefore serialized, writers of different repositories (schemas) are not.
//!   The lock is released by `COMMIT`/`ROLLBACK`, or when the connection dies,
//!   so a crashed writer can never leave it held. `READ COMMITTED` matters:
//!   every statement takes a fresh snapshot, so statements issued *after* the
//!   lock was granted see everything the previous writer committed. (A
//!   `REPEATABLE READ` transaction would pin its snapshot at the lock statement,
//!   before the lock is granted, and read stale data.)
//! * **Read transactions** run at `REPEATABLE READ READ ONLY`: one consistent
//!   snapshot, no locks, never blocked by writers.
//!
//! Failures are never silent: a lost connection is [`MetaError::ConnectionLost`]
//! (with `during_commit: true` when the outcome of a `COMMIT` is unknown), and
//! serialization failures, deadlocks and lock timeouts are
//! [`MetaError::Retriable`]. A statement that failed inside a transaction can
//! not be ignored: the transaction is rolled back instead of committed.

mod pool;
pub(crate) mod tls;
pub(crate) mod url;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use ag_types::{Limits, Timestamp};
use postgres::types::{ToSql, Type};
use sha2::{Digest, Sha256};

pub(crate) use self::pool::{Pool, Pooled};
pub(crate) use self::tls::connector as tls_connector;
pub(crate) use self::url::parse_target;
use super::db::{Conn, Dialect, Param, Row, Value, pg_placeholders};
use super::migrate::{Migration, POSTGRES_MIGRATIONS};
use super::{MetaError, Tx};
use crate::clock::Clock;

pub use self::url::url_has_password;

/// Tunables of the PostgreSQL backend.
#[derive(Clone, Debug)]
pub struct PgOptions {
    /// Maximum number of pooled connections.
    pub pool_size: usize,
    /// Timeout for opening one connection.
    pub connect_timeout: Duration,
    /// How long to wait for a free pooled connection.
    pub checkout_timeout: Duration,
    /// How long a write transaction waits for the repository's write lock
    /// (the counterpart of SQLite's `busy_timeout`). Expiry is
    /// [`MetaError::Retriable`].
    pub lock_timeout: Duration,
    /// Server-side limit for a single statement (`None`: the server default).
    pub statement_timeout: Option<Duration>,
    /// Server-side limit for a transaction that sits idle between statements.
    /// It bounds the damage of a hung client: its connection is closed, which
    /// releases the write lock.
    pub idle_in_transaction_timeout: Duration,
    /// `application_name` shown in `pg_stat_activity`.
    pub application_name: String,
    /// Create the schema (and its tables) if it does not exist. Turn off to *open an existing
    /// repository*: a missing or empty schema is then [`MetaError::NotFound`] instead of
    /// silently becoming a new, empty one (a dropped schema, a URL pointing at the wrong database).
    /// An existing schema of an older version is migrated either way.
    pub create_schema: bool,
}

impl Default for PgOptions {
    fn default() -> Self {
        Self {
            pool_size: 8,
            connect_timeout: Duration::from_secs(10),
            checkout_timeout: Duration::from_secs(30),
            lock_timeout: Duration::from_secs(30),
            statement_timeout: None,
            idle_in_transaction_timeout: Duration::from_secs(600),
            application_name: "agentgit".into(),
            create_schema: true,
        }
    }
}

/// Validates a schema name: lowercase letters, digits and `_`, starting with a
/// letter or `_`, at most 63 bytes, not reserved by PostgreSQL (`pg_` prefix).
pub fn validate_schema_name(name: &str) -> Result<(), MetaError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with("pg_")
        && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()));
    if ok {
        Ok(())
    } else {
        Err(MetaError::Invalid(format!(
            "'{name}' is not a usable schema name (lowercase letters, digits and '_'; at most 63 bytes; must not start with 'pg_' or a digit)"
        )))
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Drops a schema and everything in it (`DROP SCHEMA … CASCADE`). Irreversible;
/// for test clean-up and decommissioning a repository.
#[doc(hidden)]
pub fn drop_schema(url: &str, schema: &str) -> Result<(), MetaError> {
    validate_schema_name(schema)?;
    let target = self::url::parse_target(url)?;
    let connector = self::tls::connector(&target)?;
    let opts = PgOptions { pool_size: 1, ..PgOptions::default() };
    let pool = Pool::new(target, connector, schema, &opts);
    let mut conn = pool.get()?;
    conn.batch(&format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(schema)))
}

/// A stable 64-bit advisory-lock key for `(purpose, name)`.
pub fn advisory_key(purpose: &str, name: &str) -> i64 {
    let mut h = Sha256::new();
    h.update(b"agentgit/advisory/v1/");
    h.update(purpose.as_bytes());
    h.update(b"/");
    h.update(name.as_bytes());
    let d = h.finalize();
    i64::from_be_bytes(d[..8].try_into().expect("8 bytes"))
}

/// Maps a driver error to a [`MetaError`].
pub(crate) fn classify(e: &postgres::Error, during_commit: bool) -> MetaError {
    if let Some(db) = e.as_db_error() {
        let code = db.code().code();
        let msg = format!("{} (SQLSTATE {code})", db.message());
        return match code {
            // serialization_failure, deadlock_detected, lock_not_available, query_canceled
            "40001" | "40P01" | "55P03" | "57014" => MetaError::Retriable(msg),
            // connection_exception class, admin_shutdown, crash_shutdown, cannot_connect_now
            c if c.starts_with("08") || matches!(c, "57P01" | "57P02" | "57P03") => {
                MetaError::ConnectionLost { during_commit, message: msg }
            }
            _ => MetaError::Postgres(msg),
        };
    }
    let io = std::error::Error::source(e).is_some_and(|s| s.downcast_ref::<std::io::Error>().is_some());
    if e.is_closed() || io {
        return MetaError::ConnectionLost { during_commit, message: with_causes(e) };
    }
    MetaError::Postgres(with_causes(e))
}

/// The error and its chain of causes ("error performing TLS handshake: invalid peer certificate: UnknownIssuer").
fn with_causes(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        msg.push_str(": ");
        msg.push_str(&c.to_string());
        cur = c.source();
    }
    msg
}

/// One driver connection with its statement cache.
pub(crate) struct PgClient {
    client: postgres::Client,
    stmts: std::collections::HashMap<String, postgres::Statement>,
    /// A `BEGIN` was sent and the transaction has not ended.
    in_tx: bool,
    /// The connection may be out of sync with the server; never reuse it.
    broken: bool,
    /// When the connection was last handed back; used to validate stale ones.
    last_used: std::time::Instant,
}

enum Bound<'a> {
    I8(i64),
    I4(i32),
    I2(i16),
    Text(&'a str),
    Bytes(&'a [u8]),
    NullI8,
    NullI4,
    NullI2,
    NullText,
    NullBytes,
}

impl Bound<'_> {
    fn as_dyn(&self) -> &(dyn ToSql + Sync) {
        match self {
            Bound::I8(v) => v,
            Bound::I4(v) => v,
            Bound::I2(v) => v,
            Bound::Text(v) => v,
            Bound::Bytes(v) => v,
            Bound::NullI8 => &None::<i64>,
            Bound::NullI4 => &None::<i32>,
            Bound::NullI2 => &None::<i16>,
            Bound::NullText => &None::<&str>,
            Bound::NullBytes => &None::<&[u8]>,
        }
    }
}

fn is_text(ty: &Type) -> bool {
    *ty == Type::TEXT || *ty == Type::VARCHAR || *ty == Type::BPCHAR || *ty == Type::NAME || *ty == Type::UNKNOWN
}

fn bind<'a>(params: &'a [Param<'a>], types: &[Type]) -> Result<Vec<Bound<'a>>, MetaError> {
    if params.len() != types.len() {
        return Err(MetaError::Invalid(format!(
            "statement takes {} parameter(s) but {} were given",
            types.len(),
            params.len()
        )));
    }
    let mut out = Vec::with_capacity(params.len());
    for (i, (p, ty)) in params.iter().zip(types).enumerate() {
        let range = |what: &str| MetaError::Invalid(format!("parameter ${}: integer out of range for {what}", i + 1));
        out.push(match p {
            Param::Int(v) if *ty == Type::INT8 => Bound::I8(*v),
            Param::Int(v) if *ty == Type::INT4 => Bound::I4(i32::try_from(*v).map_err(|_| range("INT4"))?),
            Param::Int(v) if *ty == Type::INT2 => Bound::I2(i16::try_from(*v).map_err(|_| range("INT2"))?),
            Param::Text(s) if is_text(ty) => {
                if s.contains('\0') {
                    return Err(MetaError::Invalid(
                        "text containing a NUL character cannot be stored in PostgreSQL".into(),
                    ));
                }
                Bound::Text(s)
            }
            Param::Blob(b) if *ty == Type::BYTEA => Bound::Bytes(b),
            Param::Null if *ty == Type::INT8 => Bound::NullI8,
            Param::Null if *ty == Type::INT4 => Bound::NullI4,
            Param::Null if *ty == Type::INT2 => Bound::NullI2,
            Param::Null if is_text(ty) => Bound::NullText,
            Param::Null if *ty == Type::BYTEA => Bound::NullBytes,
            other => {
                return Err(MetaError::Invalid(format!("cannot bind {other:?} to parameter ${} of type {ty}", i + 1)));
            }
        });
    }
    Ok(out)
}

fn convert_row(row: &postgres::Row) -> Result<Row, MetaError> {
    let mut values = Vec::with_capacity(row.len());
    for (i, col) in row.columns().iter().enumerate() {
        let ty = col.type_();
        let bad = |e: postgres::Error| MetaError::Corrupt(format!("column {i}: {e}"));
        values.push(if *ty == Type::INT8 {
            row.try_get::<_, Option<i64>>(i).map_err(bad)?.map_or(Value::Null, Value::Int)
        } else if *ty == Type::INT4 {
            row.try_get::<_, Option<i32>>(i).map_err(bad)?.map_or(Value::Null, |v| Value::Int(i64::from(v)))
        } else if *ty == Type::INT2 {
            row.try_get::<_, Option<i16>>(i).map_err(bad)?.map_or(Value::Null, |v| Value::Int(i64::from(v)))
        } else if *ty == Type::BOOL {
            row.try_get::<_, Option<bool>>(i).map_err(bad)?.map_or(Value::Null, |v| Value::Int(i64::from(v)))
        } else if is_text(ty) {
            row.try_get::<_, Option<String>>(i).map_err(bad)?.map_or(Value::Null, Value::Text)
        } else if *ty == Type::BYTEA {
            row.try_get::<_, Option<Vec<u8>>>(i).map_err(bad)?.map_or(Value::Null, Value::Blob)
        } else {
            return Err(MetaError::Corrupt(format!("column {i} has the unsupported type {ty}")));
        });
    }
    Ok(Row::new(values))
}

impl PgClient {
    /// True if the connection can no longer be used.
    pub(crate) fn is_dead(&self) -> bool {
        self.broken || self.client.is_closed()
    }

    fn fail(&mut self, e: &postgres::Error, during_commit: bool) -> MetaError {
        let mapped = classify(e, during_commit);
        // I/O trouble, or the server ending the session (a FATAL answer such as an
        // administrator's pg_terminate_backend): the connection is of no further use.
        if e.as_db_error().is_none() || matches!(mapped, MetaError::ConnectionLost { .. }) {
            self.broken = true;
        }
        mapped
    }

    fn prepare(&mut self, sql: &str) -> Result<postgres::Statement, MetaError> {
        if let Some(s) = self.stmts.get(sql) {
            return Ok(s.clone());
        }
        let stmt = self.client.prepare(&pg_placeholders(sql)).map_err(|e| self.fail(&e, false))?;
        if self.stmts.len() >= 512 {
            self.stmts.clear();
        }
        self.stmts.insert(sql.to_string(), stmt.clone());
        Ok(stmt)
    }

    pub(crate) fn exec(&mut self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        let stmt = self.prepare(sql)?;
        let bound = bind(params, stmt.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = bound.iter().map(Bound::as_dyn).collect();
        self.client.execute(&stmt, &refs).map_err(|e| self.fail(&e, false))
    }

    pub(crate) fn fetch(&mut self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        let stmt = self.prepare(sql)?;
        let bound = bind(params, stmt.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = bound.iter().map(Bound::as_dyn).collect();
        let rows = self.client.query(&stmt, &refs).map_err(|e| self.fail(&e, false))?;
        rows.iter().map(convert_row).collect()
    }

    /// Simple-protocol statements (several, separated by `;`).
    pub(crate) fn batch(&mut self, sql: &str) -> Result<(), MetaError> {
        self.client.batch_execute(sql).map_err(|e| self.fail(&e, false))
    }

    fn abort(&mut self) {
        if self.in_tx {
            match self.client.batch_execute("ROLLBACK") {
                Ok(()) => self.in_tx = false,
                Err(_) => self.broken = true,
            }
        }
    }

    fn commit(&mut self) -> Result<(), MetaError> {
        match self.client.batch_execute("COMMIT") {
            Ok(()) => {
                self.in_tx = false;
                Ok(())
            }
            Err(e) => {
                if e.as_db_error().is_some() {
                    // The server refused the commit and rolled the transaction back.
                    self.in_tx = false;
                } else {
                    self.broken = true;
                }
                Err(classify(&e, true))
            }
        }
    }
}

/// A pooled connection idle for this long is probed before a single-statement call uses it.
const STALE_AFTER: Duration = Duration::from_secs(1);

/// How a transaction is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TxKind {
    /// Serialized by the advisory lock, `READ COMMITTED`.
    Write,
    /// `REPEATABLE READ READ ONLY`.
    Read,
    /// One statement without an explicit transaction (a statement is atomic and
    /// sees one snapshot by itself); a second statement is refused.
    Single,
}

/// The [`Conn`] handed to a transaction closure.
pub(crate) struct PgSession<'a> {
    client: RefCell<&'a mut PgClient>,
    budget: Cell<Option<u32>>,
    failed: Cell<bool>,
}

impl<'a> PgSession<'a> {
    pub(crate) fn new(client: &'a mut PgClient, budget: Option<u32>) -> Self {
        Self { client: RefCell::new(client), budget: Cell::new(budget), failed: Cell::new(false) }
    }

    fn spend(&self) -> Result<(), MetaError> {
        if let Some(n) = self.budget.get() {
            if n == 0 {
                return Err(MetaError::Invalid(
                    "a single-statement read issued a second statement; use Meta::read".into(),
                ));
            }
            self.budget.set(Some(n - 1));
        }
        Ok(())
    }

    fn note<T>(&self, r: Result<T, MetaError>) -> Result<T, MetaError> {
        if r.is_err() {
            self.failed.set(true);
        }
        r
    }
}

impl Conn for PgSession<'_> {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        self.spend()?;
        let r = self.client.borrow_mut().exec(sql, params);
        self.note(r)
    }

    fn query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        self.spend()?;
        let r = self.client.borrow_mut().fetch(sql, params);
        self.note(r)
    }
}

/// The PostgreSQL side of a [`super::Meta`].
pub(crate) struct PgBackend {
    pool: Pool,
    schema: String,
    write_key: i64,
    opts: PgOptions,
    migrations: &'static [Migration],
}

fn missing_repository(schema: &str) -> MetaError {
    MetaError::NotFound(format!(
        "PostgreSQL schema '{schema}' holds no repository (was it dropped, or does the connection URL point at the wrong database?)"
    ))
}

fn ms(d: Duration) -> u128 {
    d.as_millis().max(1)
}

impl PgBackend {
    /// Connects, creates the schema if needed and migrates it (a repository schema).
    pub(crate) fn open(url: &str, schema: &str, opts: &PgOptions, now: Timestamp) -> Result<Self, MetaError> {
        Self::open_with(url, schema, opts, now, POSTGRES_MIGRATIONS)
    }

    /// Like [`PgBackend::open`] with another migration list (a server's own tables). The first
    /// migration must create a `meta (key, value)` table: the schema version is kept there.
    pub(crate) fn open_with(
        url: &str,
        schema: &str,
        opts: &PgOptions,
        now: Timestamp,
        migrations: &'static [Migration],
    ) -> Result<Self, MetaError> {
        validate_schema_name(schema)?;
        if opts.pool_size == 0 {
            return Err(MetaError::Invalid("pool_size must be at least 1".into()));
        }
        let target = self::url::parse_target(url)?;
        let connector = self::tls::connector(&target)?;
        let pool = Pool::new(target, connector, schema, opts);
        let backend = Self {
            pool,
            schema: schema.to_string(),
            write_key: advisory_key("meta-write", schema),
            opts: opts.clone(),
            migrations,
        };
        backend.migrate(now)?;
        Ok(backend)
    }

    pub(crate) fn schema(&self) -> &str {
        &self.schema
    }

    fn migrate(&self, now: Timestamp) -> Result<(), MetaError> {
        let mut conn = self.pool.get()?;
        let key = advisory_key("meta-migrate", &self.schema);
        let c: &mut PgClient = &mut conn;
        c.in_tx = true;
        let r = (|| {
            c.batch(&format!(
                "BEGIN; SET LOCAL lock_timeout = {}; SELECT pg_advisory_xact_lock({key})",
                ms(self.opts.lock_timeout)
            ))?;
            let exists = c.fetch(
                "SELECT COUNT(*) FROM pg_catalog.pg_namespace WHERE nspname = ?1",
                &[Param::from(self.schema.as_str())],
            )?[0]
                .i64(0)?
                > 0;
            if !exists {
                if !self.opts.create_schema {
                    return Err(missing_repository(&self.schema));
                }
                c.batch(&format!("CREATE SCHEMA {}", quote_ident(&self.schema)))?;
            }
            let qualified = format!("{}.meta", quote_ident(&self.schema));
            let has_meta = c.fetch("SELECT to_regclass(?1) IS NOT NULL", &[Param::from(qualified)])?[0].i64(0)? != 0;
            let current = if has_meta {
                let rows = c.fetch("SELECT value FROM meta WHERE key = 'schema'", &[])?;
                let Some(row) = rows.first() else {
                    return Err(MetaError::Corrupt("the meta table exists but has no schema version".into()));
                };
                row.str(0)?
                    .parse::<i64>()
                    .map_err(|_| MetaError::Corrupt("the schema version in the meta table is not a number".into()))?
            } else {
                0
            };
            let latest = self.migrations.last().map_or(0, |m| m.version);
            if current > latest {
                return Err(MetaError::UnsupportedSchema { found: current, supported: latest });
            }
            if current == 0 && !self.opts.create_schema {
                return Err(missing_repository(&self.schema));
            }
            for m in self.migrations.iter().filter(|m| m.version > current) {
                c.batch(m.sql).map_err(|e| m.failed(e))?;
                c.exec(
                    "INSERT INTO meta (key, value) VALUES ('created_at', ?1) ON CONFLICT (key) DO NOTHING",
                    &[Param::from(now.as_millis().to_string())],
                )?;
                c.exec(
                    "INSERT INTO meta (key, value) VALUES ('schema', ?1)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    &[Param::from(m.version.to_string())],
                )?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => c.commit(),
            Err(e) => {
                c.abort();
                Err(e)
            }
        }
    }

    /// Opens a transaction on a pooled connection, retrying on a fresh one when
    /// the pooled connection turns out to be dead (nothing has run yet).
    fn begin(&self, kind: TxKind) -> Result<Pooled, MetaError> {
        let mut last = None;
        for _ in 0..self.opts.pool_size + 2 {
            let mut conn = self.pool.get()?;
            let sql = match kind {
                TxKind::Write => format!(
                    "BEGIN; SET LOCAL lock_timeout = {}; SELECT pg_advisory_xact_lock({})",
                    ms(self.opts.lock_timeout),
                    self.write_key
                ),
                TxKind::Read => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY".to_string(),
                TxKind::Single => {
                    // A connection that sat idle may have been cut (a failover, an administrator, a firewall):
                    // find out now, while nothing has been sent, instead of failing the caller's statement.
                    if conn.last_used.elapsed() >= STALE_AFTER
                        && let Err(e) = conn.batch("SELECT 1")
                    {
                        conn.broken = true;
                        last = Some(e);
                        continue;
                    }
                    return Ok(conn);
                }
            };
            conn.in_tx = true;
            match conn.batch(&sql) {
                Ok(()) => return Ok(conn),
                Err(e @ MetaError::ConnectionLost { .. }) => {
                    conn.broken = true;
                    last = Some(e);
                }
                Err(e) => {
                    conn.abort();
                    return Err(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| MetaError::Postgres("could not open a transaction".into())))
    }

    /// Runs `f` in a transaction of the given kind.
    pub(crate) fn run<T>(
        &self,
        kind: TxKind,
        clock: &dyn Clock,
        limits: &Limits,
        f: impl FnOnce(&Tx<'_>) -> Result<T, MetaError>,
    ) -> Result<T, MetaError> {
        // `now` is captured after the write lock was granted, like SQLite's BEGIN IMMEDIATE.
        self.run_conn(kind, |conn| f(&Tx::new(conn, clock.now(), limits)))
    }

    /// Like [`PgBackend::run`] for a closure that talks to the connection directly.
    pub(crate) fn run_conn<T>(
        &self,
        kind: TxKind,
        f: impl FnOnce(&dyn Conn) -> Result<T, MetaError>,
    ) -> Result<T, MetaError> {
        let mut conn = self.begin(kind)?;
        let out = {
            let session = PgSession::new(&mut conn, (kind == TxKind::Single).then_some(1));
            let r = f(&session);
            match r {
                Ok(_) if session.failed.get() => Err(MetaError::Postgres(
                    "a statement failed inside the transaction but its error was ignored; the transaction was rolled back"
                        .into(),
                )),
                other => other,
            }
        };
        match out {
            Ok(v) => {
                if kind != TxKind::Single {
                    conn.commit()?;
                }
                Ok(v)
            }
            Err(e) => {
                conn.abort();
                Err(e)
            }
        }
    }

    /// Number of connections currently open (for tests and diagnostics).
    pub(crate) fn open_connections(&self) -> usize {
        self.pool.total()
    }

    /// A connection of its own, outside the pool (for `LISTEN`).
    pub(crate) fn connect_dedicated(&self) -> Result<postgres::Client, MetaError> {
        self.pool.connect_dedicated()
    }
}

impl std::ops::Deref for Pooled {
    type Target = PgClient;
    fn deref(&self) -> &PgClient {
        self.client()
    }
}

impl std::ops::DerefMut for Pooled {
    fn deref_mut(&mut self) -> &mut PgClient {
        self.client_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_are_validated() {
        for ok in ["ag", "ag_acme_app", "_x", "a1", &"a".repeat(63)] {
            assert!(validate_schema_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Ag", "1a", "a-b", "a b", "a\"b", "pg_catalog", "public;drop", &"a".repeat(64), "é"] {
            assert!(validate_schema_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn advisory_keys_are_stable_and_separated() {
        assert_eq!(advisory_key("meta-write", "x"), advisory_key("meta-write", "x"));
        assert_ne!(advisory_key("meta-write", "x"), advisory_key("meta-write", "y"));
        assert_ne!(advisory_key("meta-write", "x"), advisory_key("meta-migrate", "x"));
        assert_eq!(advisory_key("meta-write", "x"), -5_386_009_446_466_365_031, "keys never change between releases");
    }
}
