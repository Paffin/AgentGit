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

use ag_integrate::{InitOptions, Repository};
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

/// Opens, creates and caches hosted repositories.
pub struct RepoManager {
    root: PathBuf,
    control: Arc<Control>,
    clock: Arc<dyn Clock>,
    pool_size: usize,
    max_open: usize,
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
    ) -> Self {
        Self {
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
            &InitOptions { namespace: namespace.clone(), default_stream: stream.clone(), actor: actor.clone() },
        );
        if let Err(e) = init {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e.into());
        }
        let row = RepoRow {
            name: name.to_string(),
            namespace: namespace.to_string(),
            default_stream: stream.to_string(),
            description: description.to_string(),
            created_at_ms: self.clock.now().as_millis(),
            created_by: actor.to_string(),
            quota,
            object_bytes: 0,
        };
        self.control.insert_repo(&row)?;
        Ok(row)
    }

    /// Returns the pool for `name`, opening the repository on first use.
    pub fn pool(&self, name: &str) -> ApiResult<Arc<RepoPool>> {
        parse_repo_name(name)?;
        if let Some(p) = self.open.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            return Ok(p.clone());
        }
        if self.control.repo(name)?.is_none() {
            return Err(ApiError::not_found(format!("repository '{name}' not found")));
        }
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
    fn repositories_are_created_once_and_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let control = Arc::new(Control::open_in_memory(clock.clone()).unwrap());
        let mgr = RepoManager::new(dir.path(), control, clock, 2, 1);
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
