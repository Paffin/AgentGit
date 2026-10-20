//! Shared fixtures for the engine tests.
//!
//! The same tests run on every backend. By default that is SQLite and the
//! filesystem (or memory); with `AG_TEST_POSTGRES_URL` the metadata moves to a
//! fresh PostgreSQL schema per test, with `AG_TEST_S3_ENDPOINT` the objects move
//! to a fresh S3 key prefix per test (see `ag_storage::testing`, and
//! `scripts/pg-test.sh` / `scripts/s3-test.sh`). With `AG_REQUIRE_BACKENDS=1` a
//! missing server fails the test instead of silently falling back.
#![allow(dead_code, missing_docs)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use ag_integrate::{
    InitOptions, IntegrateRequest, IntegrationResult, MetaConfig, ObjectsConfig, Outcome, Repository, StorageConfig,
};
use ag_storage::ManualClock;
use ag_storage::testing::{TempSchema, TestMeta, TestObjects, postgres_url, s3_endpoint};
use ag_tree::Tree;
use ag_types::{ChangesetId, FileType, Identity, Namespace, RepoPath, StreamName, Timestamp};

pub struct Fx {
    pub repo: Repository,
    pub clock: Arc<ManualClock>,
    objects: TestObjects,
    // dropped after the repository: removes the schema / key prefix
    _meta_guard: Box<dyn Send>,
}

pub fn id(s: &str) -> Identity {
    Identity::new(s).unwrap()
}

/// A repository over in-memory stores — or over PostgreSQL and/or S3 when the
/// environment asks for them.
pub fn fx() -> Fx {
    fx_with(None, None)
}

/// Like [`fx`], with the given retry policy for integrations that lose races.
pub fn fx_contended(policy: ag_integrate::ContentionPolicy) -> Fx {
    fx_with(None, Some(policy))
}

/// Like [`fx`], with the decoded-tree cache of a host on: `budget` bytes, and the cache to read its counters from.
pub fn fx_cached(budget: usize) -> (Fx, std::sync::Arc<ag_storage::DecodedTrees>) {
    let cache = ag_storage::DecodedTrees::new(budget);
    (fx_with(Some(cache.scope("acme/app")), None), cache)
}

fn fx_with(trees: Option<ag_storage::TreeScope>, policy: Option<ag_integrate::ContentionPolicy>) -> Fx {
    // Objects on S3 age on the server's clock, so the test clock must start at the real time:
    // advancing it then moves objects past a grace period exactly as it does for memory.
    let wallclock = s3_endpoint().is_some();
    let start = if wallclock { Timestamp::now() } else { Timestamp::from_millis(1_700_000_000_000) };
    let clock = Arc::new(ManualClock::new(start));
    let (meta, guard) = TestMeta::new(clock.clone()).into_parts();
    let objects = TestObjects::new(clock.clone());
    let repo = Repository::with_stores(
        Namespace::new("acme/app").unwrap(),
        StreamName::new("main").unwrap(),
        meta,
        objects.store(),
    )
    .unwrap();
    let repo = match trees {
        Some(scope) => repo.with_tree_cache(scope),
        None => repo,
    };
    let repo = match policy {
        Some(policy) => repo.with_contention_policy(policy),
        None => repo,
    };
    Fx { repo, clock, objects, _meta_guard: guard }
}

/// A temporary directory that also owns the PostgreSQL schema / S3 prefix its
/// repository uses, so they are cleaned up with it.
pub struct TestDir {
    dir: tempfile::TempDir,
    cleanup: Mutex<Vec<Box<dyn Send>>>,
}

pub fn tempdir() -> TestDir {
    TestDir { dir: tempfile::tempdir().unwrap(), cleanup: Mutex::new(Vec::new()) }
}

impl TestDir {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// The storage layout the environment selects, with fresh isolation names. The
/// configuration only names environment variables, as a real one does.
pub fn storage_for_tests(dir: &TestDir) -> Option<StorageConfig> {
    let mut storage = StorageConfig::default();
    if let Some(url) = postgres_url() {
        let schema = TempSchema::new(&url);
        storage.meta = MetaConfig::Postgres {
            url_env: Some("AG_TEST_POSTGRES_URL".into()),
            url: None,
            schema: schema.name().to_string(),
            pool_size: Some(4),
        };
        dir.cleanup.lock().unwrap().push(Box::new(schema));
    }
    #[cfg(feature = "s3")]
    if let Some(endpoint) = s3_endpoint() {
        use ag_integrate::ObjectLockKind;
        use ag_storage::testing::unique;
        let prefix = unique("agtest");
        let cfg = ag_storage::testing::s3_config(&prefix).unwrap();
        storage.objects = ObjectsConfig::S3 {
            endpoint,
            region: cfg.region.clone(),
            bucket: cfg.bucket.clone(),
            prefix: Some(prefix),
            path_style: Some(true),
            access_key_id_env: Some("AG_TEST_S3_ACCESS_KEY_ID".into()),
            secret_access_key_env: Some("AG_TEST_S3_SECRET_ACCESS_KEY".into()),
            session_token_env: Some("AG_TEST_S3_SESSION_TOKEN".into()),
            ca_bundle: None,
            lock: ObjectLockKind::Auto,
            request_timeout_secs: None,
            max_retries: None,
            list_page_size: None,
        };
        dir.cleanup.lock().unwrap().push(Box::new(ag_storage::testing::PrefixGuard::new(cfg)));
    }
    (!storage.is_default()).then_some(storage)
}

/// `.ag/` in `dir`, on the backends the environment selects.
pub fn init_repo(dir: &TestDir) -> Repository {
    init_repo_with(dir, storage_for_tests(dir))
}

pub fn init_repo_with(dir: &TestDir, storage: Option<StorageConfig>) -> Repository {
    Repository::init(
        dir.path(),
        &InitOptions {
            namespace: Namespace::new("acme/app").unwrap(),
            default_stream: StreamName::new("main").unwrap(),
            actor: id("user:alice"),
            storage,
        },
    )
    .unwrap()
}

/// True if the repository's metadata is in PostgreSQL.
pub fn meta_is_postgres(repo: &Repository) -> bool {
    repo.meta().kind() == ag_storage::meta::MetaKind::Postgres
}

/// True if the repository's objects are in an S3 bucket.
pub fn objects_are_s3(repo: &Repository) -> bool {
    matches!(repo.config().storage().objects, ObjectsConfig::S3 { .. })
}

/// Replaces the stored bytes of an object (damage simulation), on whatever backend holds it.
pub fn damage_object(repo: &Repository, hex: &str, bytes: &[u8]) {
    match repo.config().storage().objects {
        ObjectsConfig::Fs => {
            let path = repo.layout().objects_dir().join("objects").join(&hex[..2]).join(&hex[2..]);
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        #[cfg(feature = "s3")]
        ObjectsConfig::S3 { prefix, .. } => {
            let cfg = ag_storage::testing::s3_config(prefix.as_deref().unwrap()).unwrap();
            let store = ag_storage::S3ObjectStore::open(cfg, Arc::new(ag_storage::InProcessLock::new())).unwrap();
            store.corrupt_for_test(&hex.parse().unwrap(), bytes).unwrap();
        }
        #[cfg(not(feature = "s3"))]
        ObjectsConfig::S3 { .. } => unreachable!("built without the s3 feature"),
    }
}

impl Fx {
    /// True if object ages come from a server clock rather than the test clock.
    pub fn wallclock_objects(&self) -> bool {
        self.objects.is_s3()
    }

    /// Creates (but does not publish) a changeset whose complete state is `files`.
    pub fn snap(&self, parent: Option<ChangesetId>, files: &[(&str, &str)], author: &str) -> ChangesetId {
        snap_in(&self.repo, parent, files, author)
    }

    pub fn propose(&self, head: ChangesetId, title: &str, author: &str) -> String {
        self.repo.propose("main", &head, title, "", &id(author), None).unwrap().id
    }

    pub fn integrate(&self, prop: &str, actor: &str) -> IntegrationResult {
        self.repo
            .integrate(&IntegrateRequest {
                proposal_id: prop.into(),
                actor: id(actor),
                idempotency_key: None,
                fencing_tokens: vec![],
            })
            .unwrap()
    }

    pub fn head(&self) -> Option<ChangesetId> {
        self.repo.meta().read(|tx| tx.stream("main")).unwrap().unwrap().state.head
    }

    pub fn version(&self) -> u64 {
        self.repo.meta().read(|tx| tx.stream("main")).unwrap().unwrap().state.version
    }

    /// Convenience: snapshot + propose + integrate by a human.
    pub fn land(&self, parent: Option<ChangesetId>, files: &[(&str, &str)]) -> ChangesetId {
        let c = self.snap(parent, files, "user:alice");
        let p = self.propose(c, "land", "user:alice");
        let r = self.integrate(&p, "user:alice");
        assert!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
        self.head().unwrap()
    }

    pub fn read(&self, head: &ChangesetId, path: &str) -> Option<String> {
        let t = self.repo.tree_of(head).unwrap();
        t.get(&RepoPath::new(path).unwrap()).map(|e| String::from_utf8(self.repo.blob(&e.content).unwrap()).unwrap())
    }
}

pub fn snap_in(repo: &Repository, parent: Option<ChangesetId>, files: &[(&str, &str)], author: &str) -> ChangesetId {
    let base = repo.tree_or_empty(parent.as_ref()).unwrap();
    let want = Tree::from_entries(
        files.iter().map(|(p, c)| {
            (
                RepoPath::new(*p).unwrap(),
                ag_tree::Entry {
                    content: repo.put_blob(c.as_bytes().to_vec()).unwrap(),
                    file_type: FileType::Regular,
                    executable: false,
                },
            )
        }),
        repo.limits(),
    )
    .unwrap();
    repo.commit(parent.into_iter().collect(), &id(author), "snapshot", base.diff(&want)).unwrap()
}
