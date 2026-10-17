//! A small SQL database for a server's own tables (SQLite or PostgreSQL).
//!
//! [`crate::meta::Meta`] is the repository metadata store. A server also needs a few tables of its own — who may
//! do what, which repositories exist — and wants the same choice of backend: SQLite for one process, PostgreSQL
//! for several. This is the same thin, dialect-neutral seam (see [`crate::meta::db`]: `?N` placeholders,
//! `ON CONFLICT`, 64-bit integers, text and blobs) with the schema supplied by the caller:
//!
//! * **migrations** are numbered, compiled in by the caller, and applied exactly once in order. SQLite keeps the
//!   version in `PRAGMA user_version`, PostgreSQL in the `meta (key, value)` table the first migration must create
//!   (key `schema`). A database written by a newer build is refused ([`MetaError::UnsupportedSchema`]); on
//!   PostgreSQL concurrent starters serialize on an advisory lock, so any number of processes may start at once;
//! * **single statements** ([`Database::execute`], [`Database::query`]) run without an explicit transaction and take no
//!   cluster-wide lock: a statement is atomic by itself, which is all a `UPDATE … SET n = n + 1` or an
//!   `INSERT … ON CONFLICT` needs. A stale pooled connection is replaced before use;
//! * **transactions** ([`Database::read`], [`Database::write`]) are for several statements that belong together.
//!   A write transaction is serialized per schema (SQLite `BEGIN IMMEDIATE`, PostgreSQL advisory lock);
//! * **notifications** (`NOTIFY` / `LISTEN`) exist on PostgreSQL only and are best effort by nature.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use postgres::fallible_iterator::FallibleIterator;
use rusqlite::{Connection, TransactionBehavior};

use crate::clock::{Clock, SystemClock};
use crate::meta::db::{Conn, SqliteConn};
use crate::meta::pg::{PgBackend, TxKind, classify};
pub use crate::meta::{Dialect, MetaError, Migration, Param, PgOptions, Row, Value};

/// Builds a `[Param; N]` from heterogeneous values: `sql_params![id, 5i64, None::<&str>]`.
#[macro_export]
macro_rules! sql_params {
    ($($e:expr),* $(,)?) => { [$($crate::sqldb::Param::from($e)),*] };
}

/// The migrations of one schema, one list per backend. Both lists have the same length and the same versions.
#[derive(Clone, Copy, Debug)]
pub struct Migrations {
    /// SQLite flavour.
    pub sqlite: &'static [Migration],
    /// PostgreSQL flavour (its first migration creates the `meta` table).
    pub postgres: &'static [Migration],
}

impl Migrations {
    /// The schema version after all migrations.
    pub fn latest(&self) -> i64 {
        self.sqlite.last().map_or(0, |m| m.version)
    }

    /// Checks that the lists are consecutive from 1 and agree between backends. Call from a test.
    pub fn check(&self) -> Result<(), String> {
        for (name, list) in [("sqlite", self.sqlite), ("postgres", self.postgres)] {
            for (i, m) in list.iter().enumerate() {
                if m.version != i as i64 + 1 {
                    return Err(format!("{name}: migration {} is out of order (expected version {})", m.name, i + 1));
                }
                if !m.name.starts_with(&format!("{:04}_", m.version)) || m.sql.trim().is_empty() {
                    return Err(format!("{name}: migration {} is malformed", m.name));
                }
            }
        }
        let names = |l: &[Migration]| l.iter().map(|m| m.name).collect::<Vec<_>>();
        if names(self.sqlite) != names(self.postgres) {
            return Err("the SQLite and PostgreSQL migration lists disagree".into());
        }
        Ok(())
    }
}

/// Which engine a [`Database`] runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbKind {
    /// An embedded SQLite file (or memory, in tests).
    Sqlite,
    /// A PostgreSQL schema.
    Postgres,
}

enum Backend {
    Sqlite(Mutex<Connection>),
    Postgres(Box<PgBackend>),
}

/// A SQL database with a versioned schema. Cheap to share behind an `Arc`.
pub struct Database {
    backend: Backend,
    migrations: Migrations,
}

/// The statements of one transaction.
pub struct Sql<'a>(&'a dyn Conn);

impl Sql<'_> {
    /// The dialect, for the rare statement that differs.
    pub fn dialect(&self) -> Dialect {
        self.0.dialect()
    }

    /// Executes a statement and returns the number of affected rows.
    pub fn execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        self.0.execute(sql, params)
    }

    /// Runs a statement and returns all its rows.
    pub fn query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        self.0.query(sql, params)
    }

    /// The first row, if any.
    pub fn query_opt(&self, sql: &str, params: &[Param<'_>]) -> Result<Option<Row>, MetaError> {
        self.0.query_opt(sql, params)
    }
}

fn migrate_sqlite(conn: &mut Connection, list: &[Migration]) -> Result<(), MetaError> {
    let latest = list.last().map_or(0, |m| m.version);
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > latest {
        return Err(MetaError::UnsupportedSchema { found: version, supported: latest });
    }
    if version < latest {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-check under the write lock: another process may have migrated first.
        let current: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for m in list.iter().filter(|m| m.version > current) {
            tx.execute_batch(m.sql).map_err(|e| m.failed(e.into()))?;
            tx.pragma_update(None, "user_version", m.version)?;
        }
        tx.commit()?;
    }
    Ok(())
}

impl Database {
    /// Opens (creating and migrating) the SQLite database at `path`: WAL, `synchronous=FULL`, 30 s busy timeout.
    pub fn open_sqlite(path: &Path, migrations: Migrations) -> Result<Self, MetaError> {
        Self::init_sqlite(Connection::open(path)?, migrations)
    }

    /// A private in-memory SQLite database (tests).
    pub fn open_sqlite_memory(migrations: Migrations) -> Result<Self, MetaError> {
        Self::init_sqlite(Connection::open_in_memory()?, migrations)
    }

    fn init_sqlite(mut conn: Connection, migrations: Migrations) -> Result<Self, MetaError> {
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "trusted_schema", "OFF")?;
        conn.set_prepared_statement_cache_capacity(64);
        migrate_sqlite(&mut conn, migrations.sqlite)?;
        Ok(Self { backend: Backend::Sqlite(Mutex::new(conn)), migrations })
    }

    /// Opens (creating and migrating) the PostgreSQL `schema` reachable through `url`. Several processes may do
    /// this at once; a newer schema than `migrations` knows is refused.
    pub fn open_postgres(
        url: &str,
        schema: &str,
        migrations: Migrations,
        opts: &PgOptions,
        clock: &dyn Clock,
    ) -> Result<Self, MetaError> {
        let backend = PgBackend::open_with(url, schema, opts, clock.now(), migrations.postgres)?;
        Ok(Self { backend: Backend::Postgres(Box::new(backend)), migrations })
    }

    /// [`Database::open_postgres`] with the system clock.
    pub fn open_postgres_now(
        url: &str,
        schema: &str,
        migrations: Migrations,
        opts: &PgOptions,
    ) -> Result<Self, MetaError> {
        Self::open_postgres(url, schema, migrations, opts, &SystemClock)
    }

    /// The engine behind this handle.
    pub fn kind(&self) -> DbKind {
        match self.backend {
            Backend::Sqlite(_) => DbKind::Sqlite,
            Backend::Postgres(_) => DbKind::Postgres,
        }
    }

    /// The PostgreSQL schema (`None` on SQLite).
    pub fn schema_name(&self) -> Option<&str> {
        match &self.backend {
            Backend::Sqlite(_) => None,
            Backend::Postgres(p) => Some(p.schema()),
        }
    }

    /// The schema version this build migrates to.
    pub fn supported_version(&self) -> i64 {
        self.migrations.latest()
    }

    /// The schema version stored in the database right now.
    pub fn stored_version(&self) -> Result<i64, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => Ok(Self::sqlite(m).pragma_query_value(None, "user_version", |r| r.get(0))?),
            Backend::Postgres(_) => {
                let row = self.query("SELECT value FROM meta WHERE key = 'schema'", &[])?;
                let v = row.first().ok_or_else(|| MetaError::Corrupt("the meta table has no schema version".into()))?;
                v.str(0)?.parse().map_err(|_| MetaError::Corrupt("the schema version is not a number".into()))
            }
        }
    }

    fn sqlite(m: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Executes one statement (autocommit) and returns the number of affected rows.
    pub fn execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => SqliteConn(&Self::sqlite(m)).execute(sql, params),
            Backend::Postgres(p) => p.run_conn(TxKind::Single, |c| c.execute(sql, params)),
        }
    }

    /// Runs one statement (autocommit) and returns its rows.
    pub fn query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => SqliteConn(&Self::sqlite(m)).query(sql, params),
            Backend::Postgres(p) => p.run_conn(TxKind::Single, |c| c.query(sql, params)),
        }
    }

    /// Runs `f` in a read transaction (one consistent snapshot).
    pub fn read<T>(&self, f: impl FnOnce(&Sql<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => {
                let mut guard = Self::sqlite(m);
                let tx = guard.transaction_with_behavior(TransactionBehavior::Deferred)?;
                let conn = SqliteConn(&tx);
                f(&Sql(&conn))
            }
            Backend::Postgres(p) => p.run_conn(TxKind::Read, |c| f(&Sql(c))),
        }
    }

    /// Runs `f` in a write transaction: commits on `Ok`, rolls back on `Err`. Write transactions of one schema
    /// never overlap, in this process or any other.
    pub fn write<T>(&self, f: impl FnOnce(&Sql<'_>) -> Result<T, MetaError>) -> Result<T, MetaError> {
        match &self.backend {
            Backend::Sqlite(m) => {
                let mut guard = Self::sqlite(m);
                let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let out = {
                    let conn = SqliteConn(&tx);
                    f(&Sql(&conn))?
                };
                tx.commit()?;
                Ok(out)
            }
            Backend::Postgres(p) => p.run_conn(TxKind::Write, |c| f(&Sql(c))),
        }
    }

    /// Checks that the database answers.
    pub fn ping(&self) -> Result<(), MetaError> {
        self.query("SELECT 1", &[]).map(|_| ())
    }

    /// Sends a notification on `channel` (PostgreSQL `NOTIFY`; a no-op on SQLite). Best effort: a listener that
    /// is not connected at that moment never sees it.
    pub fn notify(&self, channel: &str, payload: &str) -> Result<(), MetaError> {
        check_channel(channel)?;
        match &self.backend {
            Backend::Sqlite(_) => Ok(()),
            Backend::Postgres(_) => {
                self.execute("SELECT pg_notify(?1, ?2)", &[Param::from(channel), Param::from(payload)]).map(|_| ())
            }
        }
    }

    /// A connection of its own that `LISTEN`s on `channel`; `None` on SQLite.
    pub fn listen(&self, channel: &str) -> Result<Option<Listener>, MetaError> {
        check_channel(channel)?;
        let Backend::Postgres(p) = &self.backend else { return Ok(None) };
        let mut client = p.connect_dedicated()?;
        client.batch_execute(&format!("LISTEN {channel}")).map_err(|e| classify(&e, false))?;
        Ok(Some(Listener { client, probed: Instant::now() }))
    }

    /// Number of open PostgreSQL connections (0 on SQLite). Diagnostics and tests.
    #[doc(hidden)]
    pub fn open_connections(&self) -> usize {
        match &self.backend {
            Backend::Sqlite(_) => 0,
            Backend::Postgres(p) => p.open_connections(),
        }
    }
}

fn check_channel(channel: &str) -> Result<(), MetaError> {
    if !channel.is_empty() && channel.len() <= 63 && channel.bytes().all(|b| b == b'_' || b.is_ascii_lowercase()) {
        Ok(())
    } else {
        Err(MetaError::Invalid(format!("'{channel}' is not a usable notification channel (lowercase letters and '_')")))
    }
}

/// A `LISTEN` session (PostgreSQL). Dropping it closes the connection.
pub struct Listener {
    client: postgres::Client,
    probed: Instant,
}

impl Listener {
    /// Waits up to `timeout` for notifications and returns their payloads (empty on timeout). An error means the
    /// session is gone: reconnect, and treat everything as possibly missed.
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<String>, MetaError> {
        let lost = |what: &str| MetaError::ConnectionLost { during_commit: false, message: what.into() };
        if self.client.is_closed() {
            return Err(lost("the notification connection is closed"));
        }
        let first = self.client.notifications().timeout_iter(timeout).next().map_err(|e| classify(&e, false))?;
        let mut out = Vec::new();
        if let Some(n) = first {
            out.push(n.payload().to_string());
            let mut pending = self.client.notifications();
            let mut now = pending.iter();
            while let Some(n) = now.next().map_err(|e| classify(&e, false))? {
                out.push(n.payload().to_string());
            }
        } else if self.client.is_closed() {
            return Err(lost("the server closed the notification connection"));
        }
        // A half-open TCP connection delivers nothing and reports nothing: probe now and then.
        if out.is_empty() && self.probed.elapsed() >= Duration::from_secs(20) {
            self.client.batch_execute("SELECT 1").map_err(|e| classify(&e, false))?;
            self.probed = Instant::now();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQLITE: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_t.sql",
            sql: "CREATE TABLE t (k TEXT PRIMARY KEY, n INTEGER NOT NULL) STRICT;",
        },
        Migration { version: 2, name: "0002_u.sql", sql: "ALTER TABLE t ADD COLUMN note TEXT NOT NULL DEFAULT '';" },
    ];
    const POSTGRES: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_t.sql",
            sql: "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                  CREATE TABLE t (k TEXT COLLATE \"C\" PRIMARY KEY, n BIGINT NOT NULL);",
        },
        Migration { version: 2, name: "0002_u.sql", sql: "ALTER TABLE t ADD COLUMN note TEXT NOT NULL DEFAULT '';" },
    ];
    const M: Migrations = Migrations { sqlite: SQLITE, postgres: POSTGRES };

    fn exercise(db: &Database) {
        assert_eq!(db.stored_version().unwrap(), 2);
        assert_eq!(db.supported_version(), 2);
        assert_eq!(db.execute("INSERT INTO t (k, n) VALUES (?1, ?2)", &sql_params!["a", 1i64]).unwrap(), 1);
        assert_eq!(
            db.execute("INSERT INTO t (k, n) VALUES (?1, ?2) ON CONFLICT (k) DO NOTHING", &sql_params!["a", 9i64])
                .unwrap(),
            0
        );
        assert_eq!(db.execute("UPDATE t SET n = n + ?2 WHERE k = ?1", &sql_params!["a", 4i64]).unwrap(), 1);
        let rows = db.query("SELECT n, note FROM t WHERE k = ?1", &sql_params!["a"]).unwrap();
        assert_eq!((rows[0].i64(0).unwrap(), rows[0].str(1).unwrap()), (5, ""));
        // A failed transaction leaves nothing behind.
        let r: Result<(), _> = db.write(|tx| {
            tx.execute("INSERT INTO t (k, n) VALUES (?1, ?2)", &sql_params!["b", 1i64])?;
            Err(MetaError::Invalid("boom".into()))
        });
        assert!(r.is_err());
        assert!(db.query("SELECT 1 FROM t WHERE k = 'b'", &[]).unwrap().is_empty());
        db.write(|tx| {
            tx.execute("INSERT INTO t (k, n) VALUES (?1, ?2)", &sql_params!["b", 2i64])?;
            Ok(())
        })
        .unwrap();
        let n = db.read(|tx| Ok(tx.query("SELECT COUNT(*) FROM t", &[])?[0].i64(0)?)).unwrap();
        assert_eq!(n, 2);
        db.ping().unwrap();
    }

    #[test]
    fn migration_lists_are_consistent() {
        M.check().unwrap();
        let skewed = Migrations { sqlite: SQLITE, postgres: &POSTGRES[..1] };
        assert!(skewed.check().is_err());
    }

    #[test]
    fn sqlite_runs_migrations_and_refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        exercise(&Database::open_sqlite(&path, M).unwrap());
        // Reopening is a no-op; an older build refuses what a newer one wrote.
        let older = Migrations { sqlite: &SQLITE[..1], postgres: &POSTGRES[..1] };
        match Database::open_sqlite(&path, older) {
            Err(MetaError::UnsupportedSchema { found: 2, supported: 1 }) => {}
            other => panic!("expected UnsupportedSchema, got {:?}", other.map(|_| ())),
        }
        let again = Database::open_sqlite(&path, M).unwrap();
        assert_eq!(again.stored_version().unwrap(), 2);
        assert_eq!(again.kind(), DbKind::Sqlite);
        assert!(again.listen("ag_x").unwrap().is_none());
        again.notify("ag_x", "ignored").unwrap();
        assert!(again.notify("Bad Name", "").is_err());
    }

    #[test]
    fn postgres_runs_migrations_serializes_starters_and_notifies() {
        let Some(url) = crate::testing::postgres_url() else { return };
        let schema = crate::testing::unique("agdb");
        let opts = PgOptions { pool_size: 4, ..PgOptions::default() };
        // Eight starters at once: one migrates, nobody fails.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (url, schema, opts) = (url.clone(), schema.clone(), opts.clone());
                std::thread::spawn(move || Database::open_postgres_now(&url, &schema, M, &opts).map(|_| ()))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let db = Database::open_postgres_now(&url, &schema, M, &opts).unwrap();
        assert_eq!(db.kind(), DbKind::Postgres);
        exercise(&db);

        // A schema newer than the build is refused.
        let older = Migrations { sqlite: &SQLITE[..1], postgres: &POSTGRES[..1] };
        match Database::open_postgres_now(&url, &schema, older, &opts) {
            Err(MetaError::UnsupportedSchema { found: 2, supported: 1 }) => {}
            other => panic!("expected UnsupportedSchema, got {:?}", other.map(|_| ())),
        }

        // NOTIFY reaches a listener, and several payloads arrive together.
        let mut l = db.listen("ag_test_chan").unwrap().expect("postgres listens");
        assert!(l.wait(Duration::from_millis(50)).unwrap().is_empty());
        db.notify("ag_test_chan", "one").unwrap();
        db.notify("ag_test_chan", "two").unwrap();
        let got = l.wait(Duration::from_secs(5)).unwrap();
        assert_eq!(got.first().map(String::as_str), Some("one"));
        let mut all = got;
        if all.len() < 2 {
            all.extend(l.wait(Duration::from_secs(5)).unwrap());
        }
        assert_eq!(all, ["one", "two"]);

        // A connection killed by the server is replaced before the next single statement.
        db.query("SELECT 1", &[]).unwrap();
        db.execute("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND application_name = ?1 AND state = 'idle'", &sql_params!["agentgit"]).unwrap();
        std::thread::sleep(Duration::from_millis(1300));
        for i in 0..8 {
            db.ping().unwrap_or_else(|e| panic!("statement {i} after the connections were killed: {e}"));
        }
        let _ = crate::meta::drop_schema(&url, &schema);
    }
}
