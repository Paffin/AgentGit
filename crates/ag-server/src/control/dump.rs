//! Logical backup, restore and migration of the control plane.
//!
//! A [`ControlDump`] is a JSON document holding every row of every control table, taken in one consistent snapshot.
//! Because it is logical (not a copy of a file), the same document restores into SQLite or PostgreSQL: it is the
//! backup format (`ag-server admin backup-control`), the restore format (`restore-control`) and the way a
//! single-file installation moves to PostgreSQL (`migrate-control`).
//!
//! The dump contains the *hashes* of credential secrets, so it is as sensitive as the database it came from: it is
//! written with mode 0600 and must be stored accordingly.

use std::io::Write as _;
use std::path::Path;

use ag_storage::sql_params;
use serde::{Deserialize, Serialize};

use super::{Control, db};
use crate::error::{ApiError, ApiResult};

/// Value of [`ControlDump::format`].
pub const DUMP_FORMAT: &str = "agentgit-control-backup";

/// One credential row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialDump {
    /// `cred_…`.
    pub id: String,
    /// Hex SHA-256 of the secret.
    pub secret_hash: String,
    /// Identity.
    pub subject: String,
    /// `user`, `agent`, `service`.
    pub kind: String,
    /// Description.
    pub description: String,
    /// Scope, as stored (JSON text).
    pub scope_json: String,
    /// Creation time (ms).
    pub created_at: i64,
    /// Expiry (ms).
    pub expires_at: i64,
    /// Revocation time (ms), if revoked.
    pub revoked_at: Option<i64>,
    /// Last use (ms).
    pub last_used: i64,
    /// Who minted it.
    pub created_by: String,
    /// Repository of the run the credential is bound to (empty: not bound to a run; ADR-0010).
    #[serde(default)]
    pub run_repo: String,
    /// The run the credential is bound to (empty: not bound to a run).
    #[serde(default)]
    pub run_id: String,
}

/// One role binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDump {
    /// Subject pattern.
    pub subject: String,
    /// Repository pattern.
    pub repo: String,
    /// Role name.
    pub role: String,
    /// Who granted it.
    pub created_by: String,
    /// When (ms).
    pub created_at: i64,
}

/// One registered repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDump {
    /// `owner/name`.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// First stream.
    pub default_stream: String,
    /// Description.
    pub description: String,
    /// Creation time (ms).
    pub created_at: i64,
    /// Creator.
    pub created_by: String,
    /// Quota (JSON text).
    pub quota_json: String,
    /// Tracked object bytes.
    pub object_bytes: i64,
    /// The repository's `.ag/config.json` (empty if unknown).
    #[serde(default)]
    pub config_json: String,
}

/// One recorded idempotent operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdempotencyDump {
    /// Scoped key.
    pub key: String,
    /// Hash of the request.
    pub request_hash: String,
    /// Recorded result (JSON text).
    pub result_json: String,
    /// When (ms).
    pub created_at: i64,
}

/// One server setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvDump {
    /// Key.
    pub key: String,
    /// Value.
    pub value: String,
}

/// Everything in the control plane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlDump {
    /// Always [`DUMP_FORMAT`].
    pub format: String,
    /// Layout of this document (1).
    pub version: u32,
    /// Control schema version of the database it was taken from.
    pub schema_version: i64,
    /// When the snapshot was taken (ms).
    pub created_at_ms: i64,
    /// `sqlite` or `postgres`.
    pub source: String,
    /// Credentials (secret hashes included).
    pub credentials: Vec<CredentialDump>,
    /// Role bindings.
    pub roles: Vec<RoleDump>,
    /// Registered repositories.
    pub repos: Vec<RepoDump>,
    /// Idempotency records.
    #[serde(default)]
    pub idempotency: Vec<IdempotencyDump>,
    /// Server settings.
    #[serde(default)]
    pub kv: Vec<KvDump>,
}

/// What to do with data already in the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    /// Refuse unless the target holds no credentials, roles or repositories.
    RequireEmpty,
    /// Delete everything first (in the same transaction).
    Replace,
}

/// Row counts of an import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Credentials written.
    pub credentials: usize,
    /// Role bindings written.
    pub roles: usize,
    /// Repositories written.
    pub repos: usize,
    /// Idempotency records written.
    pub idempotency: usize,
    /// Settings written.
    pub kv: usize,
}

impl ControlDump {
    /// Serializes the dump to `path` (mode 0600), atomically: a temporary file in the same directory is written,
    /// synced and renamed, so a crash never leaves a half-written backup under the final name.
    pub fn write_file(&self, path: &Path) -> ApiResult<()> {
        if path.exists() {
            return Err(ApiError::invalid(format!(
                "{} already exists; refusing to overwrite a backup",
                path.display()
            )));
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self).map_err(ApiError::internal)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp).map_err(|e| ApiError::internal(format!("cannot create {}: {e}", tmp.display())))?;
        let written = f.write_all(&body).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(ApiError::internal(format!("cannot write {}: {e}", path.display())));
        }
        Ok(())
    }

    /// Reads and validates a dump.
    pub fn read_file(path: &Path) -> ApiResult<Self> {
        let raw = std::fs::read(path).map_err(|e| ApiError::invalid(format!("cannot read {}: {e}", path.display())))?;
        let dump: Self = serde_json::from_slice(&raw)
            .map_err(|e| ApiError::invalid(format!("{} is not a control backup: {e}", path.display())))?;
        if dump.format != DUMP_FORMAT || dump.version != 1 {
            return Err(ApiError::invalid(format!(
                "{} is not a control backup this build understands (format '{}', version {})",
                path.display(),
                dump.format,
                dump.version
            )));
        }
        Ok(dump)
    }

    /// Total number of rows.
    pub fn len(&self) -> usize {
        self.credentials.len() + self.roles.len() + self.repos.len() + self.idempotency.len() + self.kv.len()
    }

    /// True if the dump holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// For rows without a stored repository configuration, takes `<data_dir>/repos/<name>/.ag/config.json` when
    /// that file exists. Returns how many rows were filled in.
    pub fn fill_repo_configs(&mut self, data_dir: &Path) -> usize {
        let mut filled = 0;
        for r in self.repos.iter_mut().filter(|r| r.config_json.is_empty()) {
            let file = data_dir.join("repos").join(&r.name).join(".ag").join("config.json");
            if let Ok(text) = std::fs::read_to_string(file) {
                r.config_json = text;
                filled += 1;
            }
        }
        filled
    }
}

impl Control {
    /// A consistent snapshot of every control table.
    pub fn export(&self) -> ApiResult<ControlDump> {
        let source = match self.kind() {
            ag_storage::sqldb::DbKind::Sqlite => "sqlite",
            ag_storage::sqldb::DbKind::Postgres => "postgres",
        };
        let (stored, _) = self.schema_versions()?;
        let mut dump = db(self.database().read(|tx| {
            let credentials = tx
                .query(
                    "SELECT id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by, run_repo, run_id
                     FROM credentials ORDER BY id",
                    &[],
                )?
                .iter()
                .map(|r| {
                    Ok(CredentialDump {
                        id: r.string(0)?,
                        secret_hash: r.string(1)?,
                        subject: r.string(2)?,
                        kind: r.string(3)?,
                        description: r.string(4)?,
                        scope_json: r.string(5)?,
                        created_at: r.i64(6)?,
                        expires_at: r.i64(7)?,
                        revoked_at: r.opt_i64(8)?,
                        last_used: r.i64(9)?,
                        created_by: r.string(10)?,
                        run_repo: r.string(11)?,
                        run_id: r.string(12)?,
                    })
                })
                .collect::<Result<Vec<_>, ag_storage::sqldb::MetaError>>()?;
            let roles = tx
                .query("SELECT subject, repo, role, created_by, created_at FROM roles ORDER BY subject, repo, role", &[])?
                .iter()
                .map(|r| {
                    Ok(RoleDump {
                        subject: r.string(0)?,
                        repo: r.string(1)?,
                        role: r.string(2)?,
                        created_by: r.string(3)?,
                        created_at: r.i64(4)?,
                    })
                })
                .collect::<Result<Vec<_>, ag_storage::sqldb::MetaError>>()?;
            let repos = tx
                .query(
                    "SELECT name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes, config_json
                     FROM repos ORDER BY name",
                    &[],
                )?
                .iter()
                .map(|r| {
                    Ok(RepoDump {
                        name: r.string(0)?,
                        namespace: r.string(1)?,
                        default_stream: r.string(2)?,
                        description: r.string(3)?,
                        created_at: r.i64(4)?,
                        created_by: r.string(5)?,
                        quota_json: r.string(6)?,
                        object_bytes: r.i64(7)?,
                        config_json: r.string(8)?,
                    })
                })
                .collect::<Result<Vec<_>, ag_storage::sqldb::MetaError>>()?;
            let idempotency = tx
                .query("SELECT key, request_hash, result_json, created_at FROM idempotency ORDER BY key", &[])?
                .iter()
                .map(|r| {
                    Ok(IdempotencyDump {
                        key: r.string(0)?,
                        request_hash: r.string(1)?,
                        result_json: r.string(2)?,
                        created_at: r.i64(3)?,
                    })
                })
                .collect::<Result<Vec<_>, ag_storage::sqldb::MetaError>>()?;
            let kv = tx
                .query("SELECT key, value FROM kv ORDER BY key", &[])?
                .iter()
                .map(|r| Ok(KvDump { key: r.string(0)?, value: r.string(1)? }))
                .collect::<Result<Vec<_>, ag_storage::sqldb::MetaError>>()?;
            Ok(ControlDump {
                format: DUMP_FORMAT.into(),
                version: 1,
                schema_version: stored,
                created_at_ms: 0,
                source: source.into(),
                credentials,
                roles,
                repos,
                idempotency,
                kv,
            })
        }))?;
        dump.created_at_ms = self.now_ms();
        Ok(dump)
    }

    /// Writes a dump into this database in one transaction: either everything is imported or nothing is.
    pub fn import(&self, dump: &ControlDump, mode: ImportMode) -> ApiResult<ImportSummary> {
        let (_, supported) = self.schema_versions()?;
        if dump.schema_version > supported {
            return Err(ApiError::invalid(format!(
                "the backup has control schema {}, newer than this build supports ({supported})",
                dump.schema_version
            )));
        }
        db(self.database().write(|tx| {
            if mode == ImportMode::RequireEmpty {
                for table in ["credentials", "roles", "repos"] {
                    let n = tx.query(&format!("SELECT COUNT(*) FROM {table}"), &[])?[0].i64(0)?;
                    if n > 0 {
                        return Err(ag_storage::sqldb::MetaError::Invalid(format!(
                            "the target control database is not empty ({n} row(s) in {table}); use --replace to overwrite it"
                        )));
                    }
                }
            } else {
                for table in ["credentials", "roles", "repos", "idempotency", "kv"] {
                    tx.execute(&format!("DELETE FROM {table}"), &[])?;
                }
            }
            for c in &dump.credentials {
                tx.execute(
                    "INSERT INTO credentials (id, secret_hash, subject, kind, description, scope_json, created_at, expires_at, revoked_at, last_used, created_by, run_repo, run_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    &sql_params![
                        &c.id,
                        &c.secret_hash,
                        &c.subject,
                        &c.kind,
                        &c.description,
                        &c.scope_json,
                        c.created_at,
                        c.expires_at,
                        c.revoked_at,
                        c.last_used,
                        &c.created_by,
                        &c.run_repo,
                        &c.run_id
                    ],
                )?;
            }
            for r in &dump.roles {
                tx.execute(
                    "INSERT INTO roles (subject, repo, role, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    &sql_params![&r.subject, &r.repo, &r.role, &r.created_by, r.created_at],
                )?;
            }
            for r in &dump.repos {
                tx.execute(
                    "INSERT INTO repos (name, namespace, default_stream, description, created_at, created_by, quota_json, object_bytes, config_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    &sql_params![
                        &r.name,
                        &r.namespace,
                        &r.default_stream,
                        &r.description,
                        r.created_at,
                        &r.created_by,
                        &r.quota_json,
                        r.object_bytes,
                        &r.config_json
                    ],
                )?;
            }
            for i in &dump.idempotency {
                tx.execute(
                    "INSERT INTO idempotency (key, request_hash, result_json, created_at) VALUES (?1, ?2, ?3, ?4)",
                    &sql_params![&i.key, &i.request_hash, &i.result_json, i.created_at],
                )?;
            }
            for k in &dump.kv {
                tx.execute("INSERT INTO kv (key, value) VALUES (?1, ?2)", &sql_params![&k.key, &k.value])?;
            }
            Ok(ImportSummary {
                credentials: dump.credentials.len(),
                roles: dump.roles.len(),
                repos: dump.repos.len(),
                idempotency: dump.idempotency.len(),
                kv: dump.kv.len(),
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use ag_protocol::v1;
    use ag_storage::SystemClock;

    use super::*;
    use crate::authz::Role;
    use crate::control::{CredentialRow, PostgresTarget, RepoRow};

    fn populated() -> Control {
        let c = Control::open_in_memory(Arc::new(SystemClock)).unwrap();
        c.insert_credential(&CredentialRow {
            id: "cred_0123456789abcdef".into(),
            secret_hash: "ab".repeat(32),
            subject: "agent:bot".into(),
            kind: "agent".into(),
            description: "ci".into(),
            scope: v1::CredentialScope {
                repos: vec!["acme/*".into()],
                streams: vec!["main".into()],
                operations: vec![],
                run_id: String::new(),
            },
            created_at_ms: 10,
            expires_at_ms: 99_999_999_999_999,
            revoked_at_ms: None,
            last_used_ms: 20,
            created_by: "user:root".into(),
        })
        .unwrap();
        c.insert_credential(&CredentialRow {
            id: "cred_fedcba9876543210".into(),
            secret_hash: "cd".repeat(32),
            subject: "user:alice".into(),
            kind: "user".into(),
            description: String::new(),
            scope: Default::default(),
            created_at_ms: 11,
            expires_at_ms: 99_999_999_999_999,
            revoked_at_ms: None,
            last_used_ms: 0,
            created_by: "user:root".into(),
        })
        .unwrap();
        assert!(c.revoke_credential("cred_fedcba9876543210", 30).unwrap());
        c.grant_role("user:alice", "acme/*", Role::Writer, "user:root").unwrap();
        c.grant_role("agent:*", "*", Role::Reader, "user:root").unwrap();
        c.insert_repo(&RepoRow {
            name: "acme/web".into(),
            namespace: "acme/web".into(),
            default_stream: "main".into(),
            description: "the web app".into(),
            created_at_ms: 5,
            created_by: "user:root".into(),
            quota: v1::Quota { max_streams: 7, ..Default::default() },
            object_bytes: 0,
            config_json: "{\"format\":1}".into(),
        })
        .unwrap();
        c.add_object_bytes("acme/web", 1234).unwrap();
        c.record_idempotent("repo.create:k1", "h", "{\"name\":\"acme/web\"}").unwrap();
        c.kv_set("note", "hello").unwrap();
        c
    }

    #[test]
    fn a_dump_restores_everything_into_sqlite_and_postgres_and_back() {
        let source = populated();
        let dump = source.export().unwrap();
        assert_eq!(
            (dump.credentials.len(), dump.roles.len(), dump.repos.len(), dump.idempotency.len(), dump.kv.len()),
            (2, 2, 1, 1, 1)
        );
        assert_eq!(dump.source, "sqlite");

        // File round trip: mode 0600, no overwrite, validated on read.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("control.json");
        dump.write_file(&file).unwrap();
        assert!(dump.write_file(&file).is_err(), "never overwrite a backup");
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777,
            0o600
        );
        let back = ControlDump::read_file(&file).unwrap();
        assert_eq!(back, dump);
        std::fs::write(dir.path().join("junk.json"), b"{\"format\":\"other\"}").unwrap();
        assert!(ControlDump::read_file(&dir.path().join("junk.json")).is_err());

        // Into a fresh SQLite database, then compare row by row via a second export.
        let sqlite = Control::open_in_memory(Arc::new(SystemClock)).unwrap();
        let summary = sqlite.import(&back, ImportMode::RequireEmpty).unwrap();
        assert_eq!(summary, ImportSummary { credentials: 2, roles: 2, repos: 1, idempotency: 1, kv: 1 });
        let again = sqlite.export().unwrap();
        assert_eq!(
            (again.credentials, again.roles, again.repos, again.idempotency, again.kv),
            (
                dump.credentials.clone(),
                dump.roles.clone(),
                dump.repos.clone(),
                dump.idempotency.clone(),
                dump.kv.clone()
            )
        );
        // A non-empty target is refused, atomically; --replace overwrites.
        assert!(sqlite.import(&back, ImportMode::RequireEmpty).is_err());
        sqlite.kv_set("extra", "x").unwrap();
        sqlite.import(&back, ImportMode::Replace).unwrap();
        assert!(sqlite.kv_get("extra").unwrap().is_none());
        assert_eq!(sqlite.repo("acme/web").unwrap().unwrap().object_bytes, 1234);

        // A dump from the future is refused.
        let mut future = back.clone();
        future.schema_version = 99;
        assert!(
            Control::open_in_memory(Arc::new(SystemClock)).unwrap().import(&future, ImportMode::RequireEmpty).is_err()
        );

        // Into PostgreSQL and back out into SQLite: the data survives both moves.
        if let Some(url) = ag_storage::testing::postgres_url() {
            let schema = ag_storage::testing::unique("agctl");
            let target = PostgresTarget {
                url: url.clone(),
                schema: schema.clone(),
                pool_size: 2,
                startup_timeout: Duration::from_secs(5),
            };
            let pg = Control::open_postgres(&target, Arc::new(SystemClock)).unwrap();
            pg.import(&back, ImportMode::RequireEmpty).unwrap();
            let from_pg = pg.export().unwrap();
            assert_eq!(from_pg.source, "postgres");
            assert_eq!(from_pg.credentials, dump.credentials);
            assert_eq!(from_pg.roles, dump.roles);
            assert_eq!(from_pg.repos, dump.repos);
            assert_eq!(from_pg.kv, dump.kv);
            assert_eq!(pg.credential("cred_fedcba9876543210").unwrap().unwrap().revoked_at_ms, Some(30));
            assert_eq!(pg.all_bindings().unwrap().len(), 2);
            let roundtrip = Control::open_in_memory(Arc::new(SystemClock)).unwrap();
            roundtrip.import(&from_pg, ImportMode::RequireEmpty).unwrap();
            assert_eq!(roundtrip.export().unwrap().repos, dump.repos);
            let _ = ag_storage::meta::drop_schema(&url, &schema);
        }
    }

    #[test]
    fn repository_configurations_are_picked_up_from_the_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ag = dir.path().join("repos/acme/web/.ag");
        std::fs::create_dir_all(&ag).unwrap();
        std::fs::write(ag.join("config.json"), "{\"format\":1,\"x\":2}").unwrap();
        let mut dump = populated().export().unwrap();
        dump.repos[0].config_json.clear();
        assert_eq!(dump.fill_repo_configs(dir.path()), 1);
        assert_eq!(dump.repos[0].config_json, "{\"format\":1,\"x\":2}");
        assert_eq!(dump.fill_repo_configs(dir.path()), 0, "already filled");
    }
}
