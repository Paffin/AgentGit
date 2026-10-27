use std::path::Path;
use std::sync::Arc;

use ag_changeset::{Changeset, ChangesetSource, Op};
use ag_merge::{BlobIo, IoError};
use ag_object::{ObjectKind, RawObject};
use ag_storage::{Clock, MemoryObjectStore, Meta, MetaTreeCache, ObjectStore, StoreHistory, SystemClock, TreeScope};
use ag_tree::{Tree, TreeCache, resolve_tree};
use ag_types::{BlobId, ChangesetId, Identity, Limits, Namespace, StreamName};
use serde_json::json;

use crate::failpoints::Failpoints;
use crate::layout::{InitOptions, Layout, RepoConfig};
use crate::storage::{self, StorageConfig};
use crate::{Error, HybridGraph, Result};

/// An open AgentGit repository.
///
/// All state-changing methods are safe to call concurrently from several
/// threads or processes: metadata changes go through database transactions
/// (SQLite or PostgreSQL) and the object store is content addressed and
/// immutable (local files or an S3-compatible bucket).
pub struct Repository {
    pub(crate) layout: Layout,
    pub(crate) config: RepoConfig,
    pub(crate) objects: Arc<dyn ObjectStore>,
    pub(crate) meta: Arc<Meta>,
    pub(crate) history: StoreHistory<Arc<dyn ObjectStore>>,
    pub(crate) limits: Limits,
    pub(crate) failpoints: Failpoints,
    /// Decoded trees kept in memory under the host's budget (none for an embedded repository).
    pub(crate) trees: Option<TreeScope>,
    /// How an integration retries when its stream keeps moving.
    pub(crate) contention: crate::integrate::ContentionPolicy,
}

impl Repository {
    /// Creates a new repository in `root`. `root` always holds `.ag/config.json`
    /// (and the local keys); with [`InitOptions::storage`] the metadata and the
    /// objects live in PostgreSQL and S3 instead of under `.ag/`.
    pub fn init(root: &Path, opts: &InitOptions) -> Result<Self> {
        let layout = Layout::new(root);
        if layout.config_path().exists() {
            return Err(Error::Conflict(format!("{} is already an AgentGit repository", root.display())));
        }
        // `Some(default)` and `None` mean the same; keep the file free of a redundant section.
        let storage = opts.storage.clone().filter(|s| !s.is_default());
        if let Some(s) = &storage {
            s.validate()?;
        }
        std::fs::create_dir_all(layout.ag_dir())?;
        let config = RepoConfig {
            format: 1,
            namespace: opts.namespace.clone(),
            default_stream: opts.default_stream.clone(),
            storage,
        };
        let repo = Self::assemble(layout.clone(), config.clone(), Arc::new(SystemClock), true)?;
        // A PostgreSQL schema or an S3 prefix can already hold a repository (initialized from
        // another directory): never initialize on top of it.
        if repo.meta.read(|tx| tx.list_streams())?.iter().any(|_| true) {
            return Err(Error::Conflict(match repo.meta.schema_name() {
                Some(s) => format!("the PostgreSQL schema '{s}' already holds a repository"),
                None => format!("{} already holds repository metadata", layout.db_path().display()),
            }));
        }
        repo.meta.write(|tx| {
            tx.create_stream(&config.default_stream, opts.actor.as_str())?;
            tx.workspace_create("default", config.default_stream.as_str(), None, opts.actor.as_str())?;
            tx.audit_append(
                opts.actor.as_str(),
                "repo.init",
                config.namespace.as_str(),
                &json!({"default_stream": config.default_stream}),
            )?;
            Ok(())
        })?;
        storage::place_guards(&layout, &config.storage())?;
        // Written last: its presence marks a fully initialized repository.
        std::fs::write(layout.config_path(), serde_json::to_vec_pretty(&config)?)?;
        Ok(repo)
    }

    /// Opens an existing repository rooted at `root`.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_clock(root, Arc::new(SystemClock))
    }

    /// Opens with an explicit clock (tests).
    pub fn open_with_clock(root: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let layout = Layout::new(root);
        let raw = std::fs::read(layout.config_path()).map_err(|_| {
            Error::Config(format!("{} is not an AgentGit repository (no .ag/config.json)", root.display()))
        })?;
        let config: RepoConfig = serde_json::from_slice(&raw)?;
        if config.format != 1 {
            return Err(Error::Config(format!("unsupported repository format {}", config.format)));
        }
        Self::assemble(layout, config, clock, false)
    }

    /// A throwaway in-memory repository for tests and embedding.
    pub fn ephemeral(namespace: Namespace, default_stream: StreamName, clock: Arc<dyn Clock>) -> Result<Self> {
        let objects: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::with_clock(clock.clone()));
        Self::with_stores(namespace, default_stream, Meta::open_in_memory(clock)?, objects)
    }

    /// A repository over stores the caller opened (a PostgreSQL [`Meta`], an S3 object
    /// store, an in-memory pair for tests…). Nothing is read from or written to disk:
    /// there is no `.ag/` directory, so [`Repository::reopen`] and [`Repository::backup`]
    /// are unavailable. The default stream and workspace are created if they do not exist
    /// yet, so calling this on an already initialized store is harmless.
    pub fn with_stores(
        namespace: Namespace,
        default_stream: StreamName,
        meta: Meta,
        objects: Arc<dyn ObjectStore>,
    ) -> Result<Self> {
        let config = RepoConfig { format: 1, namespace, default_stream, storage: None };
        let limits = meta.limits().clone();
        let repo = Self {
            layout: Layout::new(std::env::temp_dir().join("ag-ephemeral")),
            history: StoreHistory::new(objects.clone(), limits.clone()),
            config: config.clone(),
            objects,
            meta: Arc::new(meta),
            limits,
            failpoints: Failpoints::default(),
            trees: None,
            contention: Default::default(),
        };
        let actor = Identity::new("system:init").expect("valid");
        repo.meta.write(|tx| {
            if tx.stream(config.default_stream.as_str())?.is_none() {
                tx.create_stream(&config.default_stream, actor.as_str())?;
            }
            if tx.workspace("default")?.is_none() {
                tx.workspace_create("default", config.default_stream.as_str(), None, actor.as_str())?;
            }
            Ok(())
        })?;
        Ok(repo)
    }

    fn assemble(layout: Layout, config: RepoConfig, clock: Arc<dyn Clock>, create: bool) -> Result<Self> {
        let limits = Limits::default();
        let storage: StorageConfig = config.storage();
        let (meta, pg) = storage::open_meta(&storage, &layout, clock, &limits, create)?;
        let objects = storage::open_objects(&storage, &layout, config.namespace.as_str(), &limits, pg.as_ref())?;
        let meta = Arc::new(meta);
        Ok(Self {
            history: StoreHistory::new(objects.clone(), limits.clone()),
            layout,
            config,
            objects,
            meta,
            limits,
            failpoints: Failpoints::default(),
            trees: None,
            contention: Default::default(),
        })
    }

    /// Opens a second handle on the same repository (own database connections, own
    /// object-store client), e.g. for a worker thread. It re-reads the configuration,
    /// so environment variables named there must still be set.
    pub fn reopen(&self) -> Result<Self> {
        Self::open_with_clock(self.layout.root(), self.meta.clock().clone())
    }

    /// A view of this repository whose object store is `wrap(<this repository's object store>)`. The metadata
    /// store, configuration, limits and failpoints are shared with the original, so the view is as cheap as a
    /// few reference-count bumps and every write the engine makes through it passes through `wrap`'s store:
    /// the way a host accounts for (or limits) the bytes that enter a repository on behalf of one request,
    /// whichever operation writes them.
    pub fn with_object_store(&self, wrap: impl FnOnce(Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore>) -> Self {
        let objects = wrap(self.objects.clone());
        Self {
            layout: self.layout.clone(),
            config: self.config.clone(),
            history: StoreHistory::new(objects.clone(), self.limits.clone()),
            objects,
            meta: self.meta.clone(),
            limits: self.limits.clone(),
            failpoints: self.failpoints.clone(),
            trees: self.trees.clone(),
            contention: self.contention,
        }
    }

    /// Keeps the trees this repository derives in `scope` (a byte-budgeted cache the host shares between its
    /// repositories) and answers from it. A tree is a pure function of its changeset's history, so the cache cannot
    /// be stale; garbage collection empties the repository's part of it.
    #[must_use]
    pub fn with_tree_cache(mut self, scope: TreeScope) -> Self {
        self.trees = Some(scope);
        self
    }

    /// Sets how many times, and with what spacing, an integration retries when other writers keep moving its stream.
    #[must_use]
    pub fn with_contention_policy(mut self, policy: crate::integrate::ContentionPolicy) -> Self {
        self.contention = policy;
        self
    }

    /// The repository layout.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The repository configuration.
    pub fn config(&self) -> &RepoConfig {
        &self.config
    }

    /// The metadata store.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// The object store.
    pub fn objects(&self) -> &Arc<dyn ObjectStore> {
        &self.objects
    }

    /// Active resource limits.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The clock in use.
    pub fn clock(&self) -> &Arc<dyn Clock> {
        self.meta.clock()
    }

    /// Failure-injection points (tests only; never armed in production).
    pub fn failpoints(&self) -> &Failpoints {
        &self.failpoints
    }

    /// The default stream name.
    pub fn default_stream(&self) -> &StreamName {
        &self.config.default_stream
    }

    // ---- objects ---------------------------------------------------------

    /// Stores file content and returns its id.
    pub fn put_blob(&self, data: Vec<u8>) -> Result<BlobId> {
        Ok(BlobId::new(self.objects.put(&RawObject::blob(data))?))
    }

    /// Size of a stored blob in bytes without reading it (`None` if it is not stored).
    pub fn blob_len(&self, id: &BlobId) -> Result<Option<u64>> {
        Ok(self.objects.payload_len(&id.object_id())?)
    }

    /// Reads (and verifies) a blob.
    pub fn blob(&self, id: &BlobId) -> Result<Vec<u8>> {
        let raw = self.objects.get(&id.object_id())?.ok_or_else(|| Error::NotFound(format!("blob {id}")))?;
        if raw.kind != ObjectKind::Blob {
            return Err(Error::Invalid(format!("object {id} is not a blob")));
        }
        Ok(raw.payload)
    }

    /// Stores a changeset object (it is not published until [`Repository::publish`]).
    pub fn put_changeset(&self, cs: &Changeset) -> Result<ChangesetId> {
        if cs.namespace != self.config.namespace {
            return Err(Error::Invalid(format!(
                "changeset namespace '{}' does not match repository namespace '{}'",
                cs.namespace, self.config.namespace
            )));
        }
        cs.validate(&self.limits)?;
        Ok(ChangesetId::new(self.objects.put(&cs.to_raw())?))
    }

    /// Loads a changeset from the object store (verified).
    pub fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>> {
        self.history.changeset(id).map_err(|e| match e {
            ag_changeset::SourceError::NotFound(id) => Error::NotFound(format!("changeset {id}")),
            other => Error::Invalid(other.to_string()),
        })
    }

    /// Builds and stores a changeset on top of `parents` using the repository
    /// namespace and the current time.
    pub fn commit(
        &self,
        parents: Vec<ChangesetId>,
        author: &Identity,
        message: &str,
        ops: Vec<Op>,
    ) -> Result<ChangesetId> {
        let cs = Changeset::build(
            self.config.namespace.clone(),
            parents,
            author.clone(),
            self.clock().now(),
            message,
            ops,
            &self.limits,
        )?;
        self.put_changeset(&cs)
    }

    /// The tree (resulting state) of any stored changeset, published or not.
    pub fn tree_of(&self, id: &ChangesetId) -> Result<Tree> {
        if let Some(trees) = &self.trees
            && let Some(tree) = trees.get(id)
        {
            return Ok(tree);
        }
        let tree = resolve_tree(&self.history, &self.tree_cache(), id, &self.limits)?;
        if let Some(trees) = &self.trees {
            trees.put(id, &tree);
        }
        Ok(tree)
    }

    /// The tree sources `resolve_tree` starts from: memory first, then the derived trees of the metadata store.
    pub(crate) fn tree_cache(&self) -> LayeredTrees<'_> {
        LayeredTrees { meta: MetaTreeCache(&self.meta), memory: self.trees.as_ref() }
    }

    /// The tree at `id`, or the empty tree for `None`.
    pub fn tree_or_empty(&self, id: Option<&ChangesetId>) -> Result<Tree> {
        match id {
            Some(id) => self.tree_of(id),
            None => Ok(Tree::empty()),
        }
    }

    /// A DAG view that answers from the metadata index where possible and
    /// falls back to the object store for unpublished changesets.
    pub fn graph(&self) -> HybridGraph<'_> {
        HybridGraph::new(&self.meta, &self.history)
    }
}

/// In-memory decoded trees in front of the derived-tree table. A tree found in the table is remembered in memory.
pub(crate) struct LayeredTrees<'a> {
    meta: MetaTreeCache<'a>,
    memory: Option<&'a TreeScope>,
}

impl TreeCache for LayeredTrees<'_> {
    fn cached_tree(&self, id: &ChangesetId) -> Option<Tree> {
        if let Some(tree) = self.memory.and_then(|m| m.get(id)) {
            return Some(tree);
        }
        let tree = self.meta.cached_tree(id)?;
        if let Some(memory) = self.memory {
            memory.put(id, &tree);
        }
        Some(tree)
    }
}

/// Adapter so the merge engine can read and write blobs through the repository.
pub(crate) struct RepoBlobs<'a>(pub &'a Repository);

impl BlobIo for RepoBlobs<'_> {
    fn read_blob(&self, id: &BlobId) -> std::result::Result<Vec<u8>, IoError> {
        self.0.blob(id).map_err(|e| Box::new(e) as IoError)
    }

    fn write_blob(&self, data: Vec<u8>) -> std::result::Result<BlobId, IoError> {
        self.0.put_blob(data).map_err(|e| Box::new(e) as IoError)
    }
}
