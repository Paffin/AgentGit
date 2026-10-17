//! Hosting repositories: creation, opening and handle pooling.
//!
//! Every hosted repository is an ordinary AgentGit repository directory
//! (`<data_dir>/repos/<owner>/<name>/.ag`), so operators can inspect, verify
//! and back it up with the regular `ag` tooling. The server keeps a small
//! pool of engine handles per open repository (each with its own database
//! connection) and closes the least recently used repositories beyond a cap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ag_integrate::{InitOptions, MetaConfig, ObjectsConfig, RepoConfig, Repository, StorageConfig};
use ag_protocol::v1;
use ag_storage::Clock;
use ag_types::{Identity, Namespace, StreamName};

use crate::control::{Control, RepoRow};
use crate::error::{ApiError, ApiResult};

/// A few engine handles on one repository, used round-robin.
pub struct RepoPool {
    handles: Vec<Arc<Repository>>,
    next: AtomicUsize,
    last_used: Mutex<std::time::Instant>,
}

impl RepoPool {
    /// The next handle.
    pub fn get(&self) -> Arc<Repository> {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.handles.len();
        self.handles[i].clone()
    }

    fn touched(&self) -> std::time::Instant {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether a stored `.ag/config.json` puts the repository in PostgreSQL + S3, i.e. where every replica can reach it.
pub fn config_is_shared(config_json: &str) -> bool {
    serde_json::from_str::<RepoConfig>(config_json).is_ok_and(|c| {
        let s = c.storage();
        matches!(s.meta, MetaConfig::Postgres { .. }) && matches!(s.objects, ObjectsConfig::S3 { .. })
    })
}

/// The registry knows the repository but this host has none of its files and they are not in shared storage.
fn missing_local_data(name: &str) -> ApiError {
    tracing::error!(
        repo = name,
        "repository is registered but its data is not on this host and is not in PostgreSQL + S3; \
         it is served only by the replica that holds its files"
    );
    ApiError::new(
        ag_protocol::ErrorCode::Unavailable,
        format!("repository '{name}' is stored on another host's disk and cannot be served by this replica"),
    )
    .with_retry_after(1000)
}

/// Validates `owner/name` (exactly two segments) and returns the pieces.
pub fn parse_repo_name(name: &str) -> ApiResult<(&str, &str)> {
    let invalid = || {
        ApiError::invalid(format!(
            "repository name '{name}' must look like 'owner/name' (lowercase letters, digits, '.', '_', '-')"
        ))
    };
    let (owner, repo) = name.split_once('/').ok_or_else(invalid)?;
    if repo.contains('/') || Namespace::new(name).is_err() {
        return Err(invalid());
    }
    Ok((owner, repo))
}

/// Turns the server's storage template into the layout of one repository:
/// a PostgreSQL schema of its own and an S3 key prefix of its own.
pub fn derive_storage(template: &StorageConfig, repo: &str) -> StorageConfig {
    let mut out = template.clone();
    if let MetaConfig::Postgres { schema, .. } = &mut out.meta {
        let clean: String =
            repo.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        let mut name = format!("{schema}_{clean}");
        if name.len() > 60 {
            use sha2::{Digest, Sha256};
            let h = hex::encode(&Sha256::digest(name.as_bytes())[..4]);
            name.truncate(51);
            name = format!("{name}_{h}");
        }
        *schema = name;
    }
    if let ObjectsConfig::S3 { prefix, .. } = &mut out.objects {
        let base = prefix.take().unwrap_or_default();
        *prefix =
            Some(if base.is_empty() { repo.to_string() } else { format!("{}/{repo}", base.trim_end_matches('/')) });
    }
    out
}

/// Opens, creates and caches hosted repositories.
pub struct RepoManager {
    root: PathBuf,
    control: Arc<Control>,
    clock: Arc<dyn Clock>,
    pool_size: usize,
    max_open: usize,
    storage: Option<StorageConfig>,
    open: Mutex<HashMap<String, Arc<RepoPool>>>,
}

impl RepoManager {
    /// Creates a manager rooted at `<data_dir>/repos`.
    pub fn new(
        data_dir: &Path,
        control: Arc<Control>,
        clock: Arc<dyn Clock>,
        pool_size: usize,
        max_open: usize,
        storage: Option<StorageConfig>,
    ) -> Self {
        Self {
            storage,
            root: data_dir.join("repos"),
            control,
            clock,
            pool_size: pool_size.max(1),
            max_open: max_open.max(1),
            open: Mutex::new(HashMap::new()),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Creates a repository on disk and registers it.
    pub fn create(
        &self,
        name: &str,
        default_stream: &str,
        description: &str,
        actor: &Identity,
        quota: v1::Quota,
    ) -> ApiResult<RepoRow> {
        parse_repo_name(name)?;
        let stream = StreamName::new(default_stream).map_err(|e| ApiError::invalid(e.to_string()))?;
        let namespace = Namespace::new(name).map_err(|e| ApiError::invalid(e.to_string()))?;
        if self.control.repo(name)?.is_some() {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::AlreadyExists,
                format!("repository '{name}' already exists"),
            ));
        }
        let dir = self.dir(name);
        if dir.exists() {
            return Err(ApiError::internal(format!(
                "repository directory {} exists but is not registered",
                dir.display()
            )));
        }
        std::fs::create_dir_all(&dir).map_err(ApiError::internal)?;
        let init = Repository::init(
            &dir,
            &InitOptions {
                namespace: namespace.clone(),
                default_stream: stream.clone(),
                actor: actor.clone(),
                storage: self.storage.as_ref().map(|t| derive_storage(t, name)),
            },
        );
        if let Err(e) = init {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e.into());
        }
        // Registered with the exact configuration `init` wrote: any other replica can then open the repository.
        let config_json = std::fs::read_to_string(dir.join(".ag").join("config.json")).map_err(ApiError::internal)?;
        let row = RepoRow {
            name: name.to_string(),
            namespace: namespace.to_string(),
            default_stream: stream.to_string(),
            description: description.to_string(),
            created_at_ms: self.clock.now().as_millis(),
            created_by: actor.to_string(),
            quota,
            object_bytes: 0,
            config_json,
        };
        self.control.insert_repo(&row)?;
        Ok(row)
    }

    /// Makes sure `<data_dir>/repos/<name>/.ag/config.json` exists, writing it from the registry if this replica
    /// has never seen the repository. Only a repository whose metadata is in PostgreSQL and whose objects are in
    /// S3 is shared between replicas; for anything else the files exist on one host only, and writing a
    /// configuration here would create a second, *empty* repository — so that case is an error instead.
    fn ensure_local_marker(&self, row: &RepoRow) -> ApiResult<()> {
        let dir = self.dir(&row.name);
        let marker = dir.join(".ag").join("config.json");
        if marker.is_file() {
            return Ok(());
        }
        let text = if row.config_json.is_empty() {
            // Registered before the registry kept configurations: rebuild what `create` would have written.
            let template = self.storage.as_ref().ok_or_else(|| missing_local_data(&row.name))?;
            let config = RepoConfig {
                format: 1,
                namespace: Namespace::new(&row.namespace).map_err(|e| ApiError::internal(e.to_string()))?,
                default_stream: StreamName::new(&row.default_stream).map_err(|e| ApiError::internal(e.to_string()))?,
                storage: Some(derive_storage(template, &row.name)),
            };
            serde_json::to_string_pretty(&config).map_err(ApiError::internal)?
        } else {
            row.config_json.clone()
        };
        let config: RepoConfig = serde_json::from_str(&text).map_err(ApiError::internal)?;
        let storage = config.storage();
        let shared = matches!(storage.meta, MetaConfig::Postgres { .. }) && matches!(storage.objects, ObjectsConfig::S3 { .. });
        if !shared {
            return Err(missing_local_data(&row.name));
        }
        let ag = dir.join(".ag");
        std::fs::create_dir_all(&ag).map_err(ApiError::internal)?;
        // Write-then-rename: a concurrent request on this replica sees the whole file or none.
        let tmp = ag.join(format!("config.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text.as_bytes())
            .and_then(|()| std::fs::rename(&tmp, &marker))
            .map_err(ApiError::internal)?;
        tracing::info!(repo = %row.name, "opened a repository created by another replica");
        Ok(())
    }

    /// Returns the pool for `name`, opening the repository on first use.
    pub fn pool(&self, name: &str) -> ApiResult<Arc<RepoPool>> {
        parse_repo_name(name)?;
        if let Some(p) = self.open.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            return Ok(p.clone());
        }
        let Some(row) = self.control.repo(name)? else {
            return Err(ApiError::not_found(format!("repository '{name}' not found")));
        };
        self.ensure_local_marker(&row)?;
        let dir = self.dir(name);
        let first = Repository::open_with_clock(&dir, self.clock.clone())?;
        let mut handles = vec![Arc::new(first)];
        for _ in 1..self.pool_size {
            handles.push(Arc::new(handles[0].reopen()?));
        }
        let pool =
            Arc::new(RepoPool { handles, next: AtomicUsize::new(0), last_used: Mutex::new(std::time::Instant::now()) });
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        // Another request may have opened it meanwhile; keep that one.
        if let Some(p) = open.get(name) {
            return Ok(p.clone());
        }
        if open.len() >= self.max_open
            && let Some(oldest) = open.iter().min_by_key(|(_, p)| p.touched()).map(|(k, _)| k.clone())
        {
            open.remove(&oldest);
        }
        open.insert(name.to_string(), pool.clone());
        Ok(pool)
    }

    /// A handle on repository `name`.
    pub fn handle(&self, name: &str) -> ApiResult<Arc<Repository>> {
        Ok(self.pool(name)?.get())
    }

    /// Closes every open repository. Call from a blocking thread: dropping a PostgreSQL
    /// connection pool blocks, and must not happen on an async worker.
    pub fn close_all(&self) {
        let pools: Vec<_> = self.open.lock().unwrap_or_else(|e| e.into_inner()).drain().collect();
        drop(pools);
    }

    /// Number of repositories currently open.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Names of the repositories currently open.
    pub fn open_names(&self) -> Vec<String> {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use ag_storage::SystemClock;

    use super::*;

    #[test]
    fn names_are_validated_strictly() {
        assert!(parse_repo_name("acme/web").is_ok());
        for bad in ["", "acme", "acme/web/x", "Acme/web", "../etc", "a/..", "a/.b", "a b/c", "/a", "a/"] {
            assert!(parse_repo_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn storage_templates_give_every_repository_its_own_schema_and_prefix() {
        let template = StorageConfig {
            meta: MetaConfig::Postgres { url_env: None, url: None, schema: "ag".into(), pool_size: None },
            objects: ObjectsConfig::S3 {
                endpoint: "http://minio:9000".into(),
                region: "us-east-1".into(),
                bucket: "b".into(),
                prefix: Some("prod/".into()),
                path_style: None,
                access_key_id_env: None,
                secret_access_key_env: None,
                session_token_env: None,
                ca_bundle: None,
                lock: Default::default(),
            },
        };
        let a = derive_storage(&template, "acme/web");
        let b = derive_storage(&template, "acme/api");
        match (&a.meta, &a.objects, &b.meta) {
            (
                MetaConfig::Postgres { schema: sa, .. },
                ObjectsConfig::S3 { prefix, .. },
                MetaConfig::Postgres { schema: sb, .. },
            ) => {
                assert_eq!(sa, "ag_acme_web");
                assert_eq!(sb, "ag_acme_api");
                assert_eq!(prefix.as_deref(), Some("prod/acme/web"));
            }
            other => panic!("unexpected {other:?}"),
        }
        // Long names stay valid PostgreSQL identifiers and stay distinct.
        let long1 = derive_storage(&template, &format!("{}/{}", "a".repeat(40), "x".repeat(40)));
        let long2 = derive_storage(&template, &format!("{}/{}", "a".repeat(40), "y".repeat(40)));
        match (&long1.meta, &long2.meta) {
            (MetaConfig::Postgres { schema: s1, .. }, MetaConfig::Postgres { schema: s2, .. }) => {
                assert!(s1.len() <= 63 && s2.len() <= 63 && s1 != s2);
                ag_storage::meta::validate_schema_name(s1).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(derive_storage(&StorageConfig::default(), "acme/web"), StorageConfig::default());
    }

    #[test]
    fn repositories_are_created_once_and_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let control = Arc::new(Control::open_in_memory(clock.clone()).unwrap());
        let mgr = RepoManager::new(dir.path(), control, clock, 2, 1, None);
        let actor = Identity::new("user:root").unwrap();
        mgr.create("acme/web", "main", "d", &actor, Default::default()).unwrap();
        assert!(mgr.create("acme/web", "main", "d", &actor, Default::default()).is_err());
        mgr.create("acme/api", "main", "", &actor, Default::default()).unwrap();

        let repo = mgr.handle("acme/web").unwrap();
        assert_eq!(repo.config().namespace.as_str(), "acme/web");
        assert_eq!(mgr.open_count(), 1);
        // The cap evicts the least recently used repository.
        mgr.handle("acme/api").unwrap();
        assert_eq!(mgr.open_count(), 1);
        assert!(mgr.handle("acme/missing").is_err());
        assert!(dir.path().join("repos/acme/web/.ag/config.json").is_file(), "plain ag repository layout");
    }
}
