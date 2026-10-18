//! The control plane database: repositories, credentials and role bindings.
//!
//! Repository *data* lives in each repository's own metadata and object
//! stores. This small database only records what the server needs to decide
//! *who may do what to which repository*. It is SQLite in WAL mode with
//! `synchronous=FULL` by default (one server process), or one schema of a
//! PostgreSQL database when `[control] url` is set, which is what lets several
//! replicas serve the same installation (ADR-0007).
//!
//! Every mutation is a single SQL statement, atomic by itself, so replicas
//! cannot lose each other's updates and no cluster-wide lock is involved.
//! The schema is versioned (`migrations/control/`); a database written by a
//! newer build is refused, and replicas that start together migrate once.

mod dump;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_protocol::v1;
use ag_storage::sqldb::{Database, DbKind, Listener, MetaError, Migration, Migrations, Param, PgOptions, Row};
use ag_storage::{Clock, sql_params};

pub use self::dump::{ControlDump, DUMP_FORMAT, ImportMode, ImportSummary};
use crate::authz::{Binding, Role, validate_pattern};
use crate::config::ServerConfig;
use crate::error::{ApiError, ApiResult};

/// The control schema, per backend. Both lists are numbered from 1 and must agree (checked by a test).
pub const MIGRATIONS: Migrations = Migrations {
    sqlite: &[
        Migration {
            version: 1,
            name: "0001_init.sql",
            sql: include_str!("../../../../migrations/control/sqlite/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "0002_repo_config.sql",
            sql: include_str!("../../../../migrations/control/sqlite/0002_repo_config.sql"),
        },
        Migration {
            version: 3,
            name: "0003_credential_runs.sql",
            sql: include_str!("../../../../migrations/control/sqlite/0003_credential_runs.sql"),
        },
    ],
    postgres: &[
        Migration {
            version: 1,
            name: "0001_init.sql",
            sql: include_str!("../../../../migrations/control/postgres/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "0002_repo_config.sql",
            sql: include_str!("../../../../migrations/control/postgres/0002_repo_config.sql"),
        },
        Migration {
            version: 3,
            name: "0003_credential_runs.sql",
            sql: include_str!("../../../../migrations/control/postgres/0003_credential_runs.sql"),
        },
    ],
};

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
    /// The repository's `.ag/config.json` as written at creation; empty for rows registered before schema 2.
    /// It lets any replica open a repository whose storage is shared (PostgreSQL + S3).
    pub config_json: String,
}

/// Which engine holds the control plane.
pub type ControlKind = DbKind;

/// The most credentials one listing returns (newest first); the response says when there were more.
pub const MAX_LISTED_CREDENTIALS: usize = 1000;
/// The most role bindings one listing returns; the response says when there were more. (Authorization itself
/// always reads every binding.)
pub const MAX_LISTED_ROLES: usize = 5000;

/// The rows of a capped listing, and whether more existed than were returned.
#[derive(Debug)]
pub struct Listing<T> {
    /// The rows.
    pub rows: Vec<T>,
    /// True if the listing was cut at its cap.
    pub truncated: bool,
}

/// One page of the repository registry.
#[derive(Debug)]
pub struct RepoPage {
    /// The decodable rows of the page, in name order.
    pub rows: Vec<RepoRow>,
    /// The last name the query returned (the cursor for the next page), including rows that were skipped.
    pub last: Option<String>,
    /// True if the registry has nothing after `last`.
    pub exhausted: bool,
}

/// Handle to the control database.
pub struct Control {
    db: Database,
    clock: Arc<dyn Clock>,
    /// Role bindings already reported as unusable, so a bad row is logged once and not at every cache refresh.
    warned_bindings: Mutex<HashSet<String>>,
}

/// Maps a storage error: transient trouble becomes a retryable `unavailable`, the rest `internal`.
fn db<T>(r: Result<T, MetaError>) -> ApiResult<T> {
    r.map_err(|e| match e {
        MetaError::Retriable(_) | MetaError::ConnectionLost { .. } => {
            tracing::warn!(error = %e, "control database unavailable");
            ApiError::new(ag_protocol::ErrorCode::Unavailable, "the control database is temporarily unavailable; retry")
                .with_retry_after(500)
        }
        other => ApiError::internal(other),
    })
}

/// A start-up problem whose message is safe to show the operator (it never carries the connection URL).
fn startup_error(message: String) -> ApiError {
    ApiError::new(ag_protocol::ErrorCode::FailedPrecondition, message)
}

/// The control database was written by a newer build: say so, whatever the backend, instead of the opaque
/// "internal error" every other storage failure becomes.
fn newer_schema(found: i64, supported: i64) -> ApiError {
    startup_error(format!(
        "the control database has schema version {found}, newer than this server supports ({supported}); \
         upgrade ag-server (or restore an older backup) instead of running an old build against it"
    ))
}

/// Connection settings of a PostgreSQL control plane.
#[derive(Clone, Debug)]
pub struct PostgresTarget {
    /// Connection URL (may hold a password; never logged).
    pub url: String,
    /// Schema that holds the control tables.
    pub schema: String,
    /// Pooled connections.
    pub pool_size: usize,
    /// Keep retrying the first connection this long (the database may still be starting).
    pub startup_timeout: Duration,
}

fn str_param(s: &str) -> Param<'_> {
    Param::from(s)
}

impl Control {
    fn new(db: Database, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock, warned_bindings: Mutex::new(HashSet::new()) }
    }

    /// Opens (creating and migrating) the SQLite database at `path`.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        match Database::open_sqlite(path, MIGRATIONS) {
            Err(MetaError::UnsupportedSchema { found, supported }) => Err(newer_schema(found, supported)),
            other => Ok(Self::new(db(other)?, clock)),
        }
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory(clock: Arc<dyn Clock>) -> ApiResult<Self> {
        Ok(Self::new(db(Database::open_sqlite_memory(MIGRATIONS))?, clock))
    }

    /// Opens (creating and migrating) the PostgreSQL control plane. Replicas starting at the same time are safe:
    /// the migration runs once, under an advisory lock. A schema written by a newer build is refused.
    pub fn open_postgres(target: &PostgresTarget, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        let opts = PgOptions {
            pool_size: target.pool_size.max(1),
            connect_timeout: Duration::from_secs(5),
            checkout_timeout: Duration::from_secs(10),
            lock_timeout: Duration::from_secs(10),
            statement_timeout: Some(Duration::from_secs(15)),
            idle_in_transaction_timeout: Duration::from_secs(60),
            application_name: "agentgit-control".into(),
            create_schema: true,
        };
        let started = Instant::now();
        let database = loop {
            // The synchronous PostgreSQL driver drives its own small runtime, which it refuses to do on a thread
            // that already belongs to one: open on a thread of its own so callers need not care where they are.
            let attempt = std::thread::scope(|s| {
                s.spawn(|| Database::open_postgres(&target.url, &target.schema, MIGRATIONS, &opts, clock.as_ref()))
                    .join()
                    .unwrap_or_else(|_| Err(MetaError::Invalid("opening the control database panicked".into())))
            });
            match attempt {
                Ok(d) => break d,
                Err(e @ (MetaError::ConnectionLost { .. } | MetaError::Retriable(_)))
                    if started.elapsed() < target.startup_timeout =>
                {
                    tracing::warn!(error = %e, "the control database is not reachable yet; retrying");
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(MetaError::UnsupportedSchema { found, supported }) => return Err(newer_schema(found, supported)),
                Err(e) => return Err(startup_error(format!("cannot open the control database: {e}"))),
            }
        };
        Ok(Self::new(database, clock))
    }

    /// Opens the control plane the configuration selects: PostgreSQL with `[control] url`/`url_env`, otherwise
    /// the SQLite file `<data_dir>/control.db`.
    pub fn from_config(cfg: &ServerConfig, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        match cfg.control.resolve_url().map_err(|e| startup_error(e.to_string()))? {
            Some(url) => Self::open_postgres(
                &PostgresTarget {
                    url,
                    schema: cfg.control.schema.clone(),
                    pool_size: cfg.control.pool_size,
                    startup_timeout: Duration::from_secs(cfg.control.startup_timeout_secs),
                },
                clock,
            ),
            None => Self::open(&cfg.server.data_dir.join("control.db"), clock),
        }
    }

    /// The engine behind the control plane.
    pub fn kind(&self) -> ControlKind {
        self.db.kind()
    }

    /// Schema version stored in the database / the highest this build knows.
    pub fn schema_versions(&self) -> ApiResult<(i64, i64)> {
        Ok((db(self.db.stored_version())?, self.db.supported_version()))
    }

    /// Checks that the database answers (readiness).
    pub fn ping(&self) -> ApiResult<()> {
        db(self.db.ping())
    }

    /// Current time in milliseconds.
    pub fn now_ms(&self) -> i64 {
        self.clock.now().as_millis()
    }

    /// The clock in use.
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// Sends a cluster notice (PostgreSQL `NOTIFY`; a no-op on SQLite). Best effort.
    pub fn notify(&self, channel: &str, payload: &str) -> ApiResult<()> {
        db(self.db.notify(channel, payload))
    }

    /// A `LISTEN` session on `channel` (`None` on SQLite).
    pub fn listen(&self, channel: &str) -> ApiResult<Option<Listener>> {
        db(self.db.listen(channel))
    }

    // ---- credentials ------------------------------------------------------

    /// Stores a new credential.
    pub fn insert_credential(&self, c: &CredentialRow) -> ApiResult<()> {
        let scope = serde_json::to_string(&c.scope).map_err(ApiError::internal)?;
        // The run binding is repeated in two indexed columns (see migration 0003).
        let (run_repo, run_id) = match (c.scope.run_id.as_str(), c.scope.repos.as_slice()) {
            ("", _) => (String::new(), String::new()),
            (run, [repo]) => (repo.clone(), run.to_string()),
            _ => return Err(ApiError::internal("a credential bound to a run must name exactly one repository")),
        };
        db(self.db.execute(
            "INSERT INTO credentials (id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by, run_repo, run_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12)",
            &sql_params![
                &c.id,
                &c.secret_hash,
                &c.subject,
                &c.kind,
                &c.description,
                scope,
                c.created_at_ms,
                c.expires_at_ms,
                c.last_used_ms,
                &c.created_by,
                run_repo,
                run_id
            ],
        ))?;
        Ok(())
    }

    /// Decodes a credential row. A scope that does not parse is an error, never "no restriction": an unreadable
    /// row must not widen what a narrowly scoped credential may do (an empty scope means unrestricted).
    fn credential_from(r: &Row) -> Result<CredentialRow, MetaError> {
        let id = r.string(0)?;
        let scope = serde_json::from_str(r.str(5)?)
            .map_err(|e| MetaError::Corrupt(format!("credential {id}: its scope_json does not parse: {e}")))?;
        Ok(CredentialRow {
            secret_hash: r.string(1)?,
            subject: r.string(2)?,
            kind: r.string(3)?,
            description: r.string(4)?,
            scope,
            created_at_ms: r.i64(6)?,
            expires_at_ms: r.i64(7)?,
            revoked_at_ms: r.opt_i64(8)?,
            last_used_ms: r.i64(9)?,
            created_by: r.string(10)?,
            id,
        })
    }

    const CRED_COLS: &'static str = "id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by";

    /// Looks a credential up by id.
    pub fn credential(&self, id: &str) -> ApiResult<Option<CredentialRow>> {
        let sql = format!("SELECT {} FROM credentials WHERE id = ?1", Self::CRED_COLS);
        let rows = db(self.db.query(&sql, &sql_params![id]))?;
        db(rows.first().map(Self::credential_from).transpose())
    }

    /// Records a use, at most once per minute per credential.
    pub fn touch_credential(&self, id: &str, now_ms: i64) -> ApiResult<()> {
        db(self.db.execute(
            "UPDATE credentials SET last_used = CAST(?2 AS BIGINT) WHERE id = ?1 AND last_used < CAST(?2 AS BIGINT) - 60000",
            &sql_params![id, now_ms],
        ))?;
        Ok(())
    }

    /// Lists credentials, newest first, at most [`MAX_LISTED_CREDENTIALS`]; `truncated` says there were more.
    /// A row that cannot be decoded is logged and left out (it can still not authenticate anyone: looking it up
    /// by id fails), so one damaged row does not hide everybody else's credentials.
    pub fn credentials(&self, subject: Option<&str>, include_revoked: bool) -> ApiResult<Listing<CredentialRow>> {
        let sql = format!(
            "SELECT {} FROM credentials
             WHERE (CAST(?1 AS TEXT) IS NULL OR subject = CAST(?1 AS TEXT)) AND (CAST(?2 AS BIGINT) <> 0 OR revoked_at IS NULL)
             ORDER BY created_at DESC, id LIMIT {}",
            Self::CRED_COLS,
            MAX_LISTED_CREDENTIALS + 1
        );
        let rows = db(self.db.query(&sql, &sql_params![subject, include_revoked]))?;
        let truncated = rows.len() > MAX_LISTED_CREDENTIALS;
        let rows = rows
            .iter()
            .take(MAX_LISTED_CREDENTIALS)
            .filter_map(|r| match Self::credential_from(r) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::error!(error = %e, "skipping an unreadable credential row");
                    None
                }
            })
            .collect();
        Ok(Listing { rows, truncated })
    }

    /// Revokes a credential; returns whether it existed and was active.
    pub fn revoke_credential(&self, id: &str, now_ms: i64) -> ApiResult<bool> {
        let n = db(self.db.execute(
            "UPDATE credentials SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            &sql_params![id, now_ms],
        ))?;
        Ok(n > 0)
    }

    /// Revokes every active credential bound to `run_id` of `repo` (the run ended); returns how many.
    pub fn revoke_run_credentials(&self, repo: &str, run_id: &str, now_ms: i64) -> ApiResult<u64> {
        db(self.db.execute(
            "UPDATE credentials SET revoked_at = ?3 WHERE run_repo = ?1 AND run_id = ?2 AND run_id <> '' AND revoked_at IS NULL",
            &sql_params![repo, run_id, now_ms],
        ))
    }

    // ---- roles -------------------------------------------------------------

    /// Grants a role (idempotent). Both patterns must be valid globs: a typo is an error here, not a binding
    /// that is listed but can never apply.
    pub fn grant_role(&self, subject: &str, repo: &str, role: Role, by: &str) -> ApiResult<v1::RoleBinding> {
        validate_pattern(subject).map_err(|e| ApiError::invalid(format!("subject: {e}")))?;
        validate_pattern(repo).map_err(|e| ApiError::invalid(format!("repo: {e}")))?;
        let now = self.now_ms();
        db(self.db.execute(
            "INSERT INTO roles (subject, repo, role, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (subject, repo, role) DO NOTHING",
            &sql_params![subject, repo, role.as_str(), by, now],
        ))?;
        let rows = db(self.db.query(
            "SELECT created_by, created_at FROM roles WHERE subject = ?1 AND repo = ?2 AND role = ?3",
            &sql_params![subject, repo, role.as_str()],
        ))?;
        // The binding can only be missing if it was revoked between the two statements; report what was granted.
        let (created_by, created_at) = match rows.first() {
            Some(r) => (db(r.string(0))?, db(r.i64(1))?),
            None => (by.to_string(), now),
        };
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
        let n = db(self.db.execute(
            "DELETE FROM roles WHERE subject = ?1 AND repo = ?2 AND role = ?3",
            &sql_params![subject, repo, role.as_str()],
        ))?;
        Ok(n > 0)
    }

    fn role_binding_from(r: &Row) -> Result<v1::RoleBinding, MetaError> {
        Ok(v1::RoleBinding {
            subject: r.string(0)?,
            repo: r.string(1)?,
            role: r.string(2)?,
            created_by: r.string(3)?,
            created_at_ms: r.i64(4)?,
        })
    }

    /// Lists role bindings, optionally filtered, at most [`MAX_LISTED_ROLES`]; `truncated` says there were more.
    /// (The authorizer does not use this: [`Control::all_bindings`] reads every binding.)
    pub fn roles(&self, subject: Option<&str>, repo: Option<&str>) -> ApiResult<Listing<v1::RoleBinding>> {
        let rows = db(self.db.query(
            &format!(
                "SELECT subject, repo, role, created_by, created_at FROM roles
                 WHERE (CAST(?1 AS TEXT) IS NULL OR subject = CAST(?1 AS TEXT)) AND (CAST(?2 AS TEXT) IS NULL OR repo = CAST(?2 AS TEXT))
                 ORDER BY subject, repo, role LIMIT {}",
                MAX_LISTED_ROLES + 1
            ),
            &sql_params![subject, repo],
        ))?;
        let truncated = rows.len() > MAX_LISTED_ROLES;
        let rows = db(rows.iter().take(MAX_LISTED_ROLES).map(Self::role_binding_from).collect())?;
        Ok(Listing { rows, truncated })
    }

    /// Every binding — the source of every authorization decision, so it has no row limit: a binding the
    /// authorizer never saw would silently deny its subject. A row that cannot ever apply (an unknown role, a
    /// pattern that does not compile) is left out and reported once in the log.
    pub fn all_bindings(&self) -> ApiResult<Vec<Binding>> {
        let rows = db(self.db.query("SELECT subject, repo, role, created_by, created_at FROM roles", &[]))?;
        let rows: Vec<v1::RoleBinding> = db(rows.iter().map(Self::role_binding_from).collect())?;
        let mut out = Vec::with_capacity(rows.len());
        for b in rows {
            let problem = match Role::parse(&b.role) {
                None => Some(format!("unknown role '{}'", b.role)),
                Some(role) => {
                    let binding = Binding::new(b.subject.clone(), b.repo.clone(), role);
                    if binding.is_usable() {
                        out.push(binding);
                        None
                    } else {
                        Some("a pattern that does not compile".to_string())
                    }
                }
            };
            if let Some(why) = problem {
                let id = format!("{}\0{}\0{}", b.subject, b.repo, b.role);
                if self.warned_bindings.lock().unwrap_or_else(|e| e.into_inner()).insert(id) {
                    tracing::warn!(
                        subject = %b.subject, repo = %b.repo, role = %b.role,
                        "ignoring a role binding that can never apply: {why}"
                    );
                }
            }
        }
        Ok(out)
    }

    // ---- repositories --------------------------------------------------------

    /// Registers a repository.
    pub fn insert_repo(&self, r: &RepoRow) -> ApiResult<()> {
        let quota = serde_json::to_string(&r.quota).map_err(ApiError::internal)?;
        let n = db(self.db.execute(
            "INSERT INTO repos (name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes, config_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8) ON CONFLICT (name) DO NOTHING",
            &sql_params![&r.name, &r.namespace, &r.default_stream, &r.description, r.created_at_ms, &r.created_by, quota, &r.config_json],
        ))?;
        if n == 0 {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::AlreadyExists,
                format!("repository '{}' already exists", r.name),
            ));
        }
        Ok(())
    }

    /// Decodes a repository row. A quota that does not parse is an error, never "unlimited" (a default quota is
    /// all zeros, and zero means no limit): a damaged row must not switch every limit of the repository off.
    fn repo_from(r: &Row) -> Result<RepoRow, MetaError> {
        let name = r.string(0)?;
        let quota = serde_json::from_str(r.str(6)?)
            .map_err(|e| MetaError::Corrupt(format!("repository '{name}': its quota_json does not parse: {e}")))?;
        Ok(RepoRow {
            namespace: r.string(1)?,
            default_stream: r.string(2)?,
            description: r.string(3)?,
            created_at_ms: r.i64(4)?,
            created_by: r.string(5)?,
            quota,
            object_bytes: r.i64(7)?.max(0) as u64,
            config_json: r.string(8)?,
            name,
        })
    }

    const REPO_COLS: &'static str =
        "name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes, config_json";

    /// Looks a repository up.
    pub fn repo(&self, name: &str) -> ApiResult<Option<RepoRow>> {
        let sql = format!("SELECT {} FROM repos WHERE name = ?1", Self::REPO_COLS);
        let rows = db(self.db.query(&sql, &sql_params![name]))?;
        db(rows.first().map(Self::repo_from).transpose())
    }

    /// Lists repositories with names greater than `after`, in name order.
    pub fn repos(&self, prefix: &str, after: &str, limit: usize) -> ApiResult<Vec<RepoRow>> {
        Ok(self.repos_page(prefix, after, limit)?.rows)
    }

    /// One page of the registry. A listing skips (and logs) a row it cannot decode, so one damaged row cannot
    /// break it for everybody; looking that repository up by name ([`Control::repo`]) still fails loudly. The
    /// page says where it ended and whether the registry has more, so a skipped row never ends a scan early.
    pub fn repos_page(&self, prefix: &str, after: &str, limit: usize) -> ApiResult<RepoPage> {
        let sql = format!(
            "SELECT {} FROM repos WHERE name > ?1 AND substr(name, 1, length(CAST(?2 AS TEXT))) = CAST(?2 AS TEXT)
             ORDER BY name LIMIT ?3",
            Self::REPO_COLS
        );
        let raw = db(self.db.query(&sql, &sql_params![after, prefix, limit as i64]))?;
        let last = raw.last().map(|r| db(r.string(0))).transpose()?;
        let rows = raw
            .iter()
            .filter_map(|r| match Self::repo_from(r) {
                Ok(row) => Some(row),
                Err(e) => {
                    tracing::error!(error = %e, "skipping an unreadable repository row");
                    None
                }
            })
            .collect();
        Ok(RepoPage { rows, last, exhausted: raw.len() < limit })
    }

    /// Every repository, in name order. Pages through the registry, so there is no hidden cap.
    pub fn all_repos(&self) -> ApiResult<Vec<RepoRow>> {
        let mut out: Vec<RepoRow> = Vec::new();
        let mut after = String::new();
        loop {
            let page = self.repos_page("", &after, 500)?;
            out.extend(page.rows);
            match page.last {
                Some(last) if !page.exhausted => after = last,
                _ => break,
            }
        }
        Ok(out)
    }

    /// Updates mutable repository settings.
    pub fn update_repo(&self, name: &str, description: Option<&str>, quota: Option<&v1::Quota>) -> ApiResult<()> {
        if let Some(d) = description {
            db(self.db.execute("UPDATE repos SET description = ?2 WHERE name = ?1", &[str_param(name), str_param(d)]))?;
        }
        if let Some(q) = quota {
            let q = serde_json::to_string(q).map_err(ApiError::internal)?;
            db(self.db.execute("UPDATE repos SET quota_json = ?2 WHERE name = ?1", &sql_params![name, q]))?;
        }
        Ok(())
    }

    /// Adds to the tracked object bytes of a repository.
    pub fn add_object_bytes(&self, name: &str, delta: i64) -> ApiResult<()> {
        db(self.db.execute(
            "UPDATE repos SET object_bytes = CASE WHEN object_bytes + ?2 < 0 THEN 0 ELSE object_bytes + ?2 END WHERE name = ?1",
            &sql_params![name, delta],
        ))?;
        Ok(())
    }

    /// Reserves `bytes` of a repository's object budget: adds them to the tracked total unless that would pass
    /// `max_total` (0 = no limit). One conditional `UPDATE`, so concurrent writers — on this replica or another —
    /// can never reserve more than the quota between them. Returns whether the reservation was granted.
    pub fn reserve_object_bytes(&self, name: &str, bytes: u64, max_total: u64) -> ApiResult<bool> {
        let (bytes, max_total) = (bytes.min(i64::MAX as u64) as i64, max_total.min(i64::MAX as u64) as i64);
        let n = db(self.db.execute(
            "UPDATE repos SET object_bytes = object_bytes + CAST(?2 AS BIGINT)
             WHERE name = ?1 AND (CAST(?3 AS BIGINT) = 0 OR object_bytes + CAST(?2 AS BIGINT) <= CAST(?3 AS BIGINT))",
            &sql_params![name, bytes, max_total],
        ))?;
        Ok(n > 0)
    }

    /// Sets the tracked object bytes (after a recount).
    pub fn set_object_bytes(&self, name: &str, bytes: u64) -> ApiResult<()> {
        db(self.db.execute("UPDATE repos SET object_bytes = ?2 WHERE name = ?1", &sql_params![name, bytes as i64]))?;
        Ok(())
    }

    // ---- idempotency of control-plane operations ----------------------------

    /// Returns the recorded result for `key`, or an error if the key was used
    /// for a different request.
    pub fn idempotent(&self, key: &str, request_hash: &str) -> ApiResult<Option<String>> {
        if key.is_empty() {
            return Ok(None);
        }
        let rows =
            db(self.db.query("SELECT request_hash, result_json FROM idempotency WHERE key = ?1", &sql_params![key]))?;
        let Some(r) = rows.first() else { return Ok(None) };
        let (h, result) = (db(r.string(0))?, db(r.string(1))?);
        if h == request_hash {
            Ok(Some(result))
        } else {
            Err(ApiError::new(
                ag_protocol::ErrorCode::IdempotencyKeyReused,
                format!("idempotency key '{key}' was already used for a different request"),
            ))
        }
    }

    /// Records the result of a control-plane operation under `key`.
    pub fn record_idempotent(&self, key: &str, request_hash: &str, result_json: &str) -> ApiResult<()> {
        if key.is_empty() {
            return Ok(());
        }
        db(self.db.execute(
            "INSERT INTO idempotency (key, request_hash, result_json, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (key) DO NOTHING",
            &sql_params![key, request_hash, result_json, self.now_ms()],
        ))?;
        Ok(())
    }

    // ---- key/value ------------------------------------------------------------

    /// Reads a small server-level setting.
    pub fn kv_get(&self, key: &str) -> ApiResult<Option<String>> {
        let rows = db(self.db.query("SELECT value FROM kv WHERE key = ?1", &sql_params![key]))?;
        db(rows.first().map(|r| r.string(0)).transpose())
    }

    /// Writes a small server-level setting.
    pub fn kv_set(&self, key: &str, value: &str) -> ApiResult<()> {
        db(self.db.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            &sql_params![key, value],
        ))?;
        Ok(())
    }

    /// The underlying database (dump/import and tests).
    pub(crate) fn database(&self) -> &Database {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use ag_storage::SystemClock;

    use super::*;

    fn control() -> Control {
        Control::open_in_memory(Arc::new(SystemClock)).unwrap()
    }

    /// The same checks on whichever backend: SQLite always, PostgreSQL when `AG_TEST_POSTGRES_URL` is set.
    fn backends() -> Vec<(Control, Option<(String, String)>)> {
        let mut v = vec![(control(), None)];
        if let Some(url) = ag_storage::testing::postgres_url() {
            let schema = ag_storage::testing::unique("agctl");
            let target = PostgresTarget {
                url: url.clone(),
                schema: schema.clone(),
                pool_size: 4,
                startup_timeout: Duration::from_secs(5),
            };
            v.push((Control::open_postgres(&target, Arc::new(SystemClock)).unwrap(), Some((url, schema))));
        }
        v
    }

    fn cleanup(extra: Option<(String, String)>) {
        if let Some((url, schema)) = extra {
            let _ = ag_storage::meta::drop_schema(&url, &schema);
        }
    }

    fn repo_row(n: &str) -> RepoRow {
        RepoRow {
            name: n.into(),
            namespace: n.into(),
            default_stream: "main".into(),
            description: String::new(),
            created_at_ms: 1,
            created_by: "user:a".into(),
            quota: Default::default(),
            object_bytes: 0,
            config_json: String::new(),
        }
    }

    #[test]
    fn migration_lists_agree_and_every_file_is_listed() {
        MIGRATIONS.check().unwrap();
        for (backend, list) in [("sqlite", MIGRATIONS.sqlite), ("postgres", MIGRATIONS.postgres)] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/control").join(backend);
            let mut files: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n.ends_with(".sql"))
                .collect();
            files.sort();
            let listed: Vec<&str> = list.iter().map(|m| m.name).collect();
            assert_eq!(files, listed, "{backend}: migrations/control/ and the list in control/mod.rs disagree");
        }
    }

    #[test]
    fn roles_are_idempotent_and_removable() {
        for (c, extra) in backends() {
            c.grant_role("user:alice", "acme/*", Role::Writer, "user:root").unwrap();
            c.grant_role("user:alice", "acme/*", Role::Writer, "user:other").unwrap();
            assert_eq!(c.roles(Some("user:alice"), None).unwrap().rows.len(), 1);
            assert_eq!(c.roles(None, None).unwrap().rows[0].created_by, "user:root", "the first grant wins");
            assert_eq!(c.roles(Some("user:nobody"), None).unwrap().rows.len(), 0);
            assert_eq!(c.roles(None, Some("acme/*")).unwrap().rows.len(), 1);
            assert!(c.revoke_role("user:alice", "acme/*", Role::Writer).unwrap());
            assert!(!c.revoke_role("user:alice", "acme/*", Role::Writer).unwrap());
            cleanup(extra);
        }
    }

    #[test]
    fn repositories_register_once_and_list_in_order() {
        for (c, extra) in backends() {
            for n in ["b/two", "a/one", "a/three"] {
                c.insert_repo(&repo_row(n)).unwrap();
            }
            assert_eq!(c.insert_repo(&repo_row("a/one")).unwrap_err().code(), ag_protocol::ErrorCode::AlreadyExists);
            let names: Vec<_> = c.repos("", "", 10).unwrap().into_iter().map(|r| r.name).collect();
            assert_eq!(names, ["a/one", "a/three", "b/two"]);
            let names: Vec<_> = c.repos("a/", "a/one", 10).unwrap().into_iter().map(|r| r.name).collect();
            assert_eq!(names, ["a/three"]);
            c.add_object_bytes("a/one", 100).unwrap();
            c.add_object_bytes("a/one", -500).unwrap();
            assert_eq!(c.repo("a/one").unwrap().unwrap().object_bytes, 0, "never negative");
            c.add_object_bytes("a/one", 7).unwrap();
            c.add_object_bytes("a/one", 5).unwrap();
            assert_eq!(c.repo("a/one").unwrap().unwrap().object_bytes, 12);
            c.set_object_bytes("a/one", 99).unwrap();
            assert_eq!(c.repo("a/one").unwrap().unwrap().object_bytes, 99);
            c.update_repo("a/one", Some("hello"), Some(&v1::Quota { max_streams: 3, ..Default::default() })).unwrap();
            let r = c.repo("a/one").unwrap().unwrap();
            assert_eq!((r.description.as_str(), r.quota.max_streams), ("hello", 3));
            assert!(c.repo("nope/x").unwrap().is_none());
            cleanup(extra);
        }
    }

    #[test]
    fn idempotency_keys_bind_to_one_request() {
        for (c, extra) in backends() {
            assert_eq!(c.idempotent("k", "h1").unwrap(), None);
            c.record_idempotent("k", "h1", "{\"ok\":true}").unwrap();
            c.record_idempotent("k", "h2", "{\"ok\":false}").unwrap(); // the first record wins
            assert_eq!(c.idempotent("k", "h1").unwrap().as_deref(), Some("{\"ok\":true}"));
            assert_eq!(c.idempotent("k", "h2").unwrap_err().code(), ag_protocol::ErrorCode::IdempotencyKeyReused);
            assert_eq!(c.idempotent("", "h").unwrap(), None);
            cleanup(extra);
        }
    }

    #[test]
    fn credentials_round_trip_revoke_and_list() {
        for (c, extra) in backends() {
            let mk = |id: &str, subject: &str, at: i64| CredentialRow {
                id: id.into(),
                secret_hash: "ab".repeat(32),
                subject: subject.into(),
                kind: "user".into(),
                description: "d".into(),
                scope: v1::CredentialScope {
                    repos: vec!["acme/*".into()],
                    streams: vec![],
                    operations: vec![],
                    run_id: String::new(),
                },
                created_at_ms: at,
                expires_at_ms: at + 10_000,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "user:root".into(),
            };
            c.insert_credential(&mk("cred_1", "user:alice", 1)).unwrap();
            c.insert_credential(&mk("cred_2", "user:bob", 2)).unwrap();
            assert!(c.insert_credential(&mk("cred_1", "user:alice", 3)).is_err(), "ids are unique");
            let got = c.credential("cred_1").unwrap().unwrap();
            assert_eq!((got.subject.as_str(), got.scope.repos.as_slice()), ("user:alice", &["acme/*".to_string()][..]));
            assert!(c.credential("cred_x").unwrap().is_none());
            assert_eq!(c.credentials(None, false).unwrap().rows.len(), 2);
            assert_eq!(c.credentials(Some("user:bob"), false).unwrap().rows[0].id, "cred_2");
            assert!(c.revoke_credential("cred_2", 5).unwrap());
            assert!(!c.revoke_credential("cred_2", 6).unwrap(), "revoking twice does nothing");
            assert_eq!(c.credential("cred_2").unwrap().unwrap().revoked_at_ms, Some(5));
            assert_eq!(c.credentials(None, false).unwrap().rows.len(), 1);
            assert_eq!(c.credentials(None, true).unwrap().rows.len(), 2);
            // Usage is recorded at most once a minute.
            c.touch_credential("cred_1", 100_000).unwrap();
            c.touch_credential("cred_1", 100_500).unwrap();
            assert_eq!(c.credential("cred_1").unwrap().unwrap().last_used_ms, 100_000);
            c.touch_credential("cred_1", 170_000).unwrap();
            assert_eq!(c.credential("cred_1").unwrap().unwrap().last_used_ms, 170_000);
            c.kv_set("a", "1").unwrap();
            c.kv_set("a", "2").unwrap();
            assert_eq!(c.kv_get("a").unwrap().as_deref(), Some("2"));
            assert!(c.kv_get("b").unwrap().is_none());
            c.ping().unwrap();
            cleanup(extra);
        }
    }

    #[test]
    fn an_old_single_file_control_database_is_adopted_and_migrated_forward() {
        // A control.db written before the PostgreSQL control plane: schema 1 (user_version = 1), real rows.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS.sqlite[0].sql).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO repos (name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes)
                 VALUES ('old/repo', 'old/repo', 'main', 'legacy', 5, 'user:x', '{}', 42)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO credentials (id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by)
                 VALUES ('cred_0000000000000001', 'ab', 'agent:old', 'agent', 'legacy', '{\"repos\":[\"old/repo\"]}', 1, 99999999999999, NULL, 0, 'user:x')",
                [],
            )
            .unwrap();
        }
        let c = Control::open(&path, Arc::new(SystemClock)).unwrap();
        assert_eq!(c.schema_versions().unwrap(), (3, 3));
        let r = c.repo("old/repo").unwrap().unwrap();
        assert_eq!((r.description.as_str(), r.object_bytes, r.config_json.as_str()), ("legacy", 42, ""));
        // A credential from before runs could be bound is an ordinary one: bound to no run, and no run's end
        // touches it.
        let old = c.credential("cred_0000000000000001").unwrap().unwrap();
        assert!(old.scope.run_id.is_empty() && old.revoked_at_ms.is_none());
        assert_eq!(c.revoke_run_credentials("old/repo", "", 5).unwrap(), 0, "an empty run id matches nothing");
        assert_eq!(c.revoke_run_credentials("old/repo", "run_x", 5).unwrap(), 0);
        assert!(c.credential("cred_0000000000000001").unwrap().unwrap().revoked_at_ms.is_none());
        drop(c);
        // A build that only knows an older schema refuses the migrated file instead of misreading it - a
        // run-bound credential must never be read as one that is bound to the agent's name only.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn transient_database_trouble_is_a_retryable_unavailable() {
        let e = db::<()>(Err(MetaError::Retriable("busy".into()))).unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::Unavailable);
        assert!(e.0.retry_after_ms > 0);
        let e = db::<()>(Err(MetaError::ConnectionLost { during_commit: false, message: "gone".into() })).unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::Unavailable);
        assert_eq!(db::<()>(Err(MetaError::Corrupt("x".into()))).unwrap_err().code(), ag_protocol::ErrorCode::Internal);
    }

    fn credential_row(id: &str, scope: v1::CredentialScope) -> CredentialRow {
        CredentialRow {
            id: id.into(),
            secret_hash: "ab".repeat(32),
            subject: "agent:bot".into(),
            kind: "agent".into(),
            description: String::new(),
            scope,
            created_at_ms: 1,
            expires_at_ms: i64::MAX,
            revoked_at_ms: None,
            last_used_ms: 0,
            created_by: "user:root".into(),
        }
    }

    #[test]
    fn an_unreadable_scope_or_quota_is_an_error_never_a_wider_grant() {
        let c = control();
        let narrow = v1::CredentialScope { repos: vec!["acme/web".into()], ..Default::default() };
        c.insert_credential(&credential_row("cred_1", narrow.clone())).unwrap();
        c.insert_credential(&credential_row("cred_2", narrow)).unwrap();
        c.insert_repo(&repo_row("acme/web")).unwrap();
        c.insert_repo(&repo_row("acme/api")).unwrap();
        assert_eq!(c.credential("cred_1").unwrap().unwrap().scope.repos, ["acme/web"]);

        // Damage rows the way a bad restore or a hand edit would.
        let bad_scope = "UPDATE credentials SET scope_json = 'not json' WHERE id = 'cred_1'";
        c.database().execute(bad_scope, &[]).unwrap();
        let bad_quota = "UPDATE repos SET quota_json = '{\"max_streams\": \"many\"}' WHERE name = 'acme/web'";
        c.database().execute(bad_quota, &[]).unwrap();

        // The old code answered with the default scope (= unrestricted) and the default quota (= unlimited).
        let e = c.credential("cred_1").unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::Internal, "fails closed: {e:?}");
        assert!(c.repo("acme/web").is_err(), "a repository with an unreadable quota is not served unlimited");
        // One damaged row does not hide the others from a listing; it is skipped (and logged).
        let listed = c.credentials(None, false).unwrap();
        assert_eq!(listed.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["cred_2"]);
        assert_eq!(c.repos("", "", 10).unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["acme/api"]);
        assert_eq!(c.all_repos().unwrap().len(), 1);
    }

    #[test]
    fn every_role_binding_reaches_the_authorizer_however_many_there_are() {
        let c = control();
        let insert = "INSERT INTO roles (subject, repo, role, created_by, created_at) VALUES (?1, ?2, ?3, 'test', 1)";
        c.database()
            .write(|tx| {
                for i in 0..6000 {
                    tx.execute(insert, &sql_params![format!("user:u{i:05}"), "acme/*", "reader"])?;
                }
                // Rows that can never apply: an unknown role, a pattern that does not compile.
                tx.execute(insert, &sql_params!["user:zed", "acme/*", "overlord"])?;
                tx.execute(insert, &sql_params!["user:zed", "acme/[", "reader"])?;
                Ok(())
            })
            .unwrap();
        let all = c.all_bindings().unwrap();
        assert_eq!(all.len(), 6000, "the old query stopped at 5000 and silently dropped the rest");
        assert!(all.iter().any(|b| b.subject == "user:u05999"), "the last binding in subject order is honoured");
        assert!(all.iter().all(|b| b.is_usable()));
        // Listing is a capped view, and says so.
        let listed = c.roles(None, None).unwrap();
        assert_eq!(listed.rows.len(), MAX_LISTED_ROLES);
        assert!(listed.truncated);
        let one = c.roles(Some("user:u00001"), None).unwrap();
        assert_eq!((one.rows.len(), one.truncated), (1, false));
    }

    #[test]
    fn a_role_can_only_be_granted_on_patterns_that_compile() {
        let c = control();
        c.grant_role("user:alice", "acme/*", Role::Writer, "user:root").unwrap();
        for (subject, repo) in [("user:alice", "acme/["), ("user:[", "acme/*"), ("", "acme/*"), ("user:alice", "")] {
            let e = c.grant_role(subject, repo, Role::Writer, "user:root").unwrap_err();
            assert_eq!(e.code(), ag_protocol::ErrorCode::InvalidArgument, "{subject:?} {repo:?}");
        }
        assert_eq!(c.roles(None, None).unwrap().rows.len(), 1, "nothing was stored for the rejected ones");
    }

    #[test]
    fn credential_listings_say_when_they_are_cut_short() {
        let c = control();
        c.database()
            .write(|tx| {
                for i in 0..(MAX_LISTED_CREDENTIALS + 5) {
                    tx.execute(
                        "INSERT INTO credentials (id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, last_used, created_by)
                         VALUES (?1, 'ab', 'user:a', 'user', '', '{}', ?2, 9, 0, 'x')",
                        &sql_params![format!("cred_{i:05}"), i as i64],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let all = c.credentials(None, false).unwrap();
        assert_eq!((all.rows.len(), all.truncated), (MAX_LISTED_CREDENTIALS, true));
        assert_eq!(all.rows[0].id, format!("cred_{:05}", MAX_LISTED_CREDENTIALS + 4), "newest first");
        assert!(!c.credentials(Some("user:nobody"), false).unwrap().truncated);
    }

    #[test]
    fn the_repository_scan_pages_through_every_row() {
        let c = control();
        c.database()
            .write(|tx| {
                for i in 0..1203 {
                    tx.execute(
                        "INSERT INTO repos (name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes, config_json)
                         VALUES (?1, ?1, 'main', '', 1, 'x', '{}', 0, '')",
                        &sql_params![format!("acme/r{i:05}")],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        c.database().execute("UPDATE repos SET quota_json = 'oops' WHERE name = 'acme/r00007'", &[]).unwrap();
        let all = c.all_repos().unwrap();
        assert_eq!(all.len(), 1202, "all but the damaged row, across three pages");
        assert_eq!(all.last().unwrap().name, "acme/r01202");
        // A page that skipped a row is still not "the end": the cursor moves past the skipped name.
        let page = c.repos_page("", "", 10).unwrap();
        assert_eq!((page.rows.len(), page.last.as_deref(), page.exhausted), (9, Some("acme/r00009"), false));
    }

    #[test]
    fn object_bytes_are_reserved_atomically_against_the_quota() {
        for (c, extra) in backends() {
            c.insert_repo(&repo_row("a/one")).unwrap();
            assert!(c.reserve_object_bytes("a/one", 60, 100).unwrap());
            assert!(!c.reserve_object_bytes("a/one", 41, 100).unwrap(), "60 + 41 passes the quota of 100");
            assert!(c.reserve_object_bytes("a/one", 40, 100).unwrap(), "exactly the quota is allowed");
            assert_eq!(c.repo("a/one").unwrap().unwrap().object_bytes, 100);
            assert!(c.reserve_object_bytes("a/one", 1_000_000, 0).unwrap(), "0 = unlimited");
            assert!(!c.reserve_object_bytes("a/missing", 1, 0).unwrap(), "nothing to reserve in");
            cleanup(extra);
        }
    }
}
