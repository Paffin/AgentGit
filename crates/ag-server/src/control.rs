//! The control plane database: repositories, credentials and role bindings.
//!
//! Repository *data* lives in each repository's own metadata and object
//! stores. This small database only records what the server needs to decide
//! *who may do what to which repository*. It is SQLite in WAL mode with
//! `synchronous=FULL`, like the per-repository metadata store.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ag_protocol::v1;
use ag_storage::Clock;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::authz::{Binding, Role};
use crate::error::{ApiError, ApiResult};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
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
";

/// A stored credential (the secret itself is never stored, only its hash).
#[derive(Clone, Debug)]
pub struct CredentialRow {
    /// `cred_<16 hex>`.
    pub id: String,
    /// Lowercase hex SHA-256 of the secret part of the token.
    pub secret_hash: String,
    /// Identity the credential authenticates as.
    pub subject: String,
    /// `user`, `agent` or `service`.
    pub kind: String,
    /// Free-form description.
    pub description: String,
    /// Scope as stored.
    pub scope: v1::CredentialScope,
    /// Creation time.
    pub created_at_ms: i64,
    /// Expiry time.
    pub expires_at_ms: i64,
    /// Revocation time.
    pub revoked_at_ms: Option<i64>,
    /// Last use (updated at most once a minute).
    pub last_used_ms: i64,
    /// Who minted it.
    pub created_by: String,
}

impl CredentialRow {
    /// The public wire form (no secret material).
    pub fn to_wire(&self) -> v1::Credential {
        v1::Credential {
            id: self.id.clone(),
            subject: self.subject.clone(),
            kind: self.kind.clone(),
            description: self.description.clone(),
            scope: Some(self.scope.clone()),
            created_at_ms: self.created_at_ms,
            expires_at_ms: self.expires_at_ms,
            revoked_at_ms: self.revoked_at_ms.unwrap_or(0),
            last_used_ms: self.last_used_ms,
            created_by: self.created_by.clone(),
        }
    }
}

/// A registered repository.
#[derive(Clone, Debug)]
pub struct RepoRow {
    /// `owner/name`.
    pub name: String,
    /// Namespace hashed into changesets.
    pub namespace: String,
    /// First stream.
    pub default_stream: String,
    /// Description.
    pub description: String,
    /// Creation time.
    pub created_at_ms: i64,
    /// Creator.
    pub created_by: String,
    /// Quota.
    pub quota: v1::Quota,
    /// Tracked object bytes (approximate; recomputed by maintenance).
    pub object_bytes: u64,
}

/// Handle to the control database.
pub struct Control {
    conn: Mutex<Connection>,
    clock: Arc<dyn Clock>,
}

fn db<T>(r: rusqlite::Result<T>) -> ApiResult<T> {
    r.map_err(ApiError::internal)
}

impl Control {
    /// Opens (creating and migrating) the database at `path`.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        let conn = db(Connection::open(path))?;
        Self::init(conn, clock)
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory(clock: Arc<dyn Clock>) -> ApiResult<Self> {
        Self::init(db(Connection::open_in_memory())?, clock)
    }

    fn init(conn: Connection, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        db(conn.busy_timeout(Duration::from_secs(30)))?;
        db(conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0)))?;
        db(conn.pragma_update(None, "synchronous", "FULL"))?;
        db(conn.pragma_update(None, "trusted_schema", "OFF"))?;
        let mut me = Self { conn: Mutex::new(conn), clock };
        me.migrate()?;
        Ok(me)
    }

    fn migrate(&mut self) -> ApiResult<()> {
        let conn = self.conn.get_mut().unwrap_or_else(|e| e.into_inner());
        let version: i64 = db(conn.pragma_query_value(None, "user_version", |r| r.get(0)))?;
        if version > SCHEMA_VERSION {
            return Err(ApiError::internal(format!(
                "control database schema {version} is newer than this server supports ({SCHEMA_VERSION})"
            )));
        }
        if version == 0 {
            let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let current: i64 = db(tx.pragma_query_value(None, "user_version", |r| r.get(0)))?;
            if current == 0 {
                db(tx.execute_batch(SCHEMA))?;
                db(tx.pragma_update(None, "user_version", SCHEMA_VERSION))?;
            }
            db(tx.commit())?;
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Current time in milliseconds.
    pub fn now_ms(&self) -> i64 {
        self.clock.now().as_millis()
    }

    // ---- credentials ------------------------------------------------------

    /// Stores a new credential.
    pub fn insert_credential(&self, c: &CredentialRow) -> ApiResult<()> {
        let scope = serde_json::to_string(&c.scope).map_err(ApiError::internal)?;
        db(self.lock().execute(
            "INSERT INTO credentials (id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10)",
            params![c.id, c.secret_hash, c.subject, c.kind, c.description, scope, c.created_at_ms, c.expires_at_ms, c.last_used_ms, c.created_by],
        ))?;
        Ok(())
    }

    fn row_to_credential(r: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRow> {
        let scope_json: String = r.get(5)?;
        Ok(CredentialRow {
            id: r.get(0)?,
            secret_hash: r.get(1)?,
            subject: r.get(2)?,
            kind: r.get(3)?,
            description: r.get(4)?,
            scope: serde_json::from_str(&scope_json).unwrap_or_default(),
            created_at_ms: r.get(6)?,
            expires_at_ms: r.get(7)?,
            revoked_at_ms: r.get(8)?,
            last_used_ms: r.get(9)?,
            created_by: r.get(10)?,
        })
    }

    const CRED_COLS: &'static str = "id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by";

    /// Looks a credential up by id.
    pub fn credential(&self, id: &str) -> ApiResult<Option<CredentialRow>> {
        let sql = format!("SELECT {} FROM credentials WHERE id = ?1", Self::CRED_COLS);
        db(self.lock().query_row(&sql, [id], Self::row_to_credential).optional())
    }

    /// Records a use, at most once per minute per credential.
    pub fn touch_credential(&self, id: &str, now_ms: i64) -> ApiResult<()> {
        db(self.lock().execute(
            "UPDATE credentials SET last_used = ?2 WHERE id = ?1 AND last_used < ?2 - 60000",
            params![id, now_ms],
        ))?;
        Ok(())
    }

    /// Lists credentials, newest first.
    pub fn credentials(&self, subject: Option<&str>, include_revoked: bool) -> ApiResult<Vec<CredentialRow>> {
        let sql = format!(
            "SELECT {} FROM credentials WHERE (?1 IS NULL OR subject = ?1) AND (?2 OR revoked_at IS NULL)
             ORDER BY created_at DESC, id LIMIT 1000",
            Self::CRED_COLS
        );
        let conn = self.lock();
        let mut stmt = db(conn.prepare(&sql))?;
        let rows = db(stmt.query_map(params![subject, include_revoked], Self::row_to_credential))?;
        db(rows.collect())
    }

    /// Revokes a credential; returns whether it existed and was active.
    pub fn revoke_credential(&self, id: &str, now_ms: i64) -> ApiResult<bool> {
        let n = db(self.lock().execute(
            "UPDATE credentials SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_ms],
        ))?;
        Ok(n > 0)
    }

    // ---- roles -------------------------------------------------------------

    /// Grants a role (idempotent).
    pub fn grant_role(&self, subject: &str, repo: &str, role: Role, by: &str) -> ApiResult<v1::RoleBinding> {
        let now = self.now_ms();
        db(self.lock().execute(
            "INSERT INTO roles (subject, repo, role, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(subject, repo, role) DO NOTHING",
            params![subject, repo, role.as_str(), by, now],
        ))?;
        let (created_by, created_at): (String, i64) = db(self.lock().query_row(
            "SELECT created_by, created_at FROM roles WHERE subject = ?1 AND repo = ?2 AND role = ?3",
            params![subject, repo, role.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        Ok(v1::RoleBinding {
            subject: subject.into(),
            repo: repo.into(),
            role: role.as_str().into(),
            created_by,
            created_at_ms: created_at,
        })
    }

    /// Revokes a role binding; returns whether it existed.
    pub fn revoke_role(&self, subject: &str, repo: &str, role: Role) -> ApiResult<bool> {
        let n = db(self.lock().execute(
            "DELETE FROM roles WHERE subject = ?1 AND repo = ?2 AND role = ?3",
            params![subject, repo, role.as_str()],
        ))?;
        Ok(n > 0)
    }

    /// Lists role bindings, optionally filtered.
    pub fn roles(&self, subject: Option<&str>, repo: Option<&str>) -> ApiResult<Vec<v1::RoleBinding>> {
        let conn = self.lock();
        let mut stmt = db(conn.prepare(
            "SELECT subject, repo, role, created_by, created_at FROM roles
             WHERE (?1 IS NULL OR subject = ?1) AND (?2 IS NULL OR repo = ?2) ORDER BY subject, repo, role LIMIT 5000",
        ))?;
        let rows = db(stmt.query_map(params![subject, repo], |r| {
            Ok(v1::RoleBinding {
                subject: r.get(0)?,
                repo: r.get(1)?,
                role: r.get(2)?,
                created_by: r.get(3)?,
                created_at_ms: r.get(4)?,
            })
        }))?;
        db(rows.collect())
    }

    /// Every binding (the authorizer filters by pattern). Bindings are few;
    /// callers cache the result briefly.
    pub fn all_bindings(&self) -> ApiResult<Vec<Binding>> {
        Ok(self
            .roles(None, None)?
            .into_iter()
            .filter_map(|b| Some(Binding { subject: b.subject, repo: b.repo, role: Role::parse(&b.role)? }))
            .collect())
    }

    // ---- repositories --------------------------------------------------------

    /// Registers a repository.
    pub fn insert_repo(&self, r: &RepoRow) -> ApiResult<()> {
        let quota = serde_json::to_string(&r.quota).map_err(ApiError::internal)?;
        let res = self.lock().execute(
            "INSERT INTO repos (name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
            params![r.name, r.namespace, r.default_stream, r.description, r.created_at_ms, r.created_by, quota],
        );
        match res {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => Err(
                ApiError::new(ag_protocol::ErrorCode::AlreadyExists, format!("repository '{}' already exists", r.name)),
            ),
            Err(e) => Err(ApiError::internal(e)),
        }
    }

    fn row_to_repo(r: &rusqlite::Row<'_>) -> rusqlite::Result<RepoRow> {
        let quota: String = r.get(6)?;
        Ok(RepoRow {
            name: r.get(0)?,
            namespace: r.get(1)?,
            default_stream: r.get(2)?,
            description: r.get(3)?,
            created_at_ms: r.get(4)?,
            created_by: r.get(5)?,
            quota: serde_json::from_str(&quota).unwrap_or_default(),
            object_bytes: r.get::<_, i64>(7)?.max(0) as u64,
        })
    }

    const REPO_COLS: &'static str =
        "name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes";

    /// Looks a repository up.
    pub fn repo(&self, name: &str) -> ApiResult<Option<RepoRow>> {
        let sql = format!("SELECT {} FROM repos WHERE name = ?1", Self::REPO_COLS);
        db(self.lock().query_row(&sql, [name], Self::row_to_repo).optional())
    }

    /// Lists repositories with names greater than `after`, in name order.
    pub fn repos(&self, prefix: &str, after: &str, limit: usize) -> ApiResult<Vec<RepoRow>> {
        let sql = format!(
            "SELECT {} FROM repos WHERE name > ?1 AND substr(name, 1, length(?2)) = ?2 ORDER BY name LIMIT ?3",
            Self::REPO_COLS
        );
        let conn = self.lock();
        let mut stmt = db(conn.prepare(&sql))?;
        let rows = db(stmt.query_map(params![after, prefix, limit as i64], Self::row_to_repo))?;
        db(rows.collect())
    }

    /// Updates mutable repository settings.
    pub fn update_repo(&self, name: &str, description: Option<&str>, quota: Option<&v1::Quota>) -> ApiResult<()> {
        let conn = self.lock();
        if let Some(d) = description {
            db(conn.execute("UPDATE repos SET description = ?2 WHERE name = ?1", params![name, d]))?;
        }
        if let Some(q) = quota {
            let q = serde_json::to_string(q).map_err(ApiError::internal)?;
            db(conn.execute("UPDATE repos SET quota_json = ?2 WHERE name = ?1", params![name, q]))?;
        }
        Ok(())
    }

    /// Adds to the tracked object bytes of a repository.
    pub fn add_object_bytes(&self, name: &str, delta: i64) -> ApiResult<()> {
        db(self.lock().execute(
            "UPDATE repos SET object_bytes = MAX(0, object_bytes + ?2) WHERE name = ?1",
            params![name, delta],
        ))?;
        Ok(())
    }

    /// Sets the tracked object bytes (after a recount).
    pub fn set_object_bytes(&self, name: &str, bytes: u64) -> ApiResult<()> {
        db(self.lock().execute("UPDATE repos SET object_bytes = ?2 WHERE name = ?1", params![name, bytes as i64]))?;
        Ok(())
    }

    // ---- idempotency of control-plane operations ----------------------------

    /// Returns the recorded result for `key`, or an error if the key was used
    /// for a different request.
    pub fn idempotent(&self, key: &str, request_hash: &str) -> ApiResult<Option<String>> {
        if key.is_empty() {
            return Ok(None);
        }
        let row: Option<(String, String)> = db(self
            .lock()
            .query_row("SELECT request_hash, result_json FROM idempotency WHERE key = ?1", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional())?;
        match row {
            None => Ok(None),
            Some((h, result)) if h == request_hash => Ok(Some(result)),
            Some(_) => Err(ApiError::new(
                ag_protocol::ErrorCode::IdempotencyKeyReused,
                format!("idempotency key '{key}' was already used for a different request"),
            )),
        }
    }

    /// Records the result of a control-plane operation under `key`.
    pub fn record_idempotent(&self, key: &str, request_hash: &str, result_json: &str) -> ApiResult<()> {
        if key.is_empty() {
            return Ok(());
        }
        db(self.lock().execute(
            "INSERT INTO idempotency (key, request_hash, result_json, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO NOTHING",
            params![key, request_hash, result_json, self.now_ms()],
        ))?;
        Ok(())
    }

    // ---- key/value ------------------------------------------------------------

    /// Reads a small server-level setting.
    pub fn kv_get(&self, key: &str) -> ApiResult<Option<String>> {
        db(self.lock().query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional())
    }

    /// Writes a small server-level setting.
    pub fn kv_set(&self, key: &str, value: &str) -> ApiResult<()> {
        db(self.lock().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ag_storage::SystemClock;

    use super::*;

    fn control() -> Control {
        Control::open_in_memory(Arc::new(SystemClock)).unwrap()
    }

    #[test]
    fn roles_are_idempotent_and_removable() {
        let c = control();
        c.grant_role("user:alice", "acme/*", Role::Writer, "user:root").unwrap();
        c.grant_role("user:alice", "acme/*", Role::Writer, "user:other").unwrap();
        assert_eq!(c.roles(Some("user:alice"), None).unwrap().len(), 1);
        assert_eq!(c.roles(None, None).unwrap()[0].created_by, "user:root", "the first grant wins");
        assert!(c.revoke_role("user:alice", "acme/*", Role::Writer).unwrap());
        assert!(!c.revoke_role("user:alice", "acme/*", Role::Writer).unwrap());
    }

    #[test]
    fn repositories_register_once_and_list_in_order() {
        let c = control();
        let mk = |n: &str| RepoRow {
            name: n.into(),
            namespace: n.into(),
            default_stream: "main".into(),
            description: String::new(),
            created_at_ms: 1,
            created_by: "user:a".into(),
            quota: Default::default(),
            object_bytes: 0,
        };
        for n in ["b/two", "a/one", "a/three"] {
            c.insert_repo(&mk(n)).unwrap();
        }
        assert_eq!(c.insert_repo(&mk("a/one")).unwrap_err().code(), ag_protocol::ErrorCode::AlreadyExists);
        let names: Vec<_> = c.repos("", "", 10).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["a/one", "a/three", "b/two"]);
        let names: Vec<_> = c.repos("a/", "a/one", 10).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["a/three"]);
        c.add_object_bytes("a/one", 100).unwrap();
        c.add_object_bytes("a/one", -500).unwrap();
        assert_eq!(c.repo("a/one").unwrap().unwrap().object_bytes, 0, "never negative");
    }

    #[test]
    fn idempotency_keys_bind_to_one_request() {
        let c = control();
        assert_eq!(c.idempotent("k", "h1").unwrap(), None);
        c.record_idempotent("k", "h1", "{\"ok\":true}").unwrap();
        assert_eq!(c.idempotent("k", "h1").unwrap().as_deref(), Some("{\"ok\":true}"));
        assert_eq!(c.idempotent("k", "h2").unwrap_err().code(), ag_protocol::ErrorCode::IdempotencyKeyReused);
    }
}
