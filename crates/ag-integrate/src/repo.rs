use std::path::Path;
use std::sync::Arc;

use ag_changeset::{Changeset, ChangesetSource, Op};
use ag_merge::{BlobIo, IoError};
use ag_object::{ObjectKind, RawObject};
use ag_storage::{
    Clock, FsObjectStore, MemoryObjectStore, Meta, MetaTreeCache, ObjectStore, StoreHistory, SystemClock,
};
use ag_tree::{Tree, resolve_tree};
use ag_types::{BlobId, ChangesetId, Identity, Limits, Namespace, StreamName};
use serde_json::json;

use crate::failpoints::Failpoints;
use crate::layout::{InitOptions, Layout, RepoConfig};
use crate::{Error, HybridGraph, Result};

/// An open AgentGit repository.
///
/// All state-changing methods are safe to call concurrently from several
/// threads or processes: metadata changes go through SQLite transactions and
/// the object store is content addressed and immutable.
pub struct Repository {
    pub(crate) layout: Layout,
    pub(crate) config: RepoConfig,
    pub(crate) objects: Arc<dyn ObjectStore>,
    pub(crate) meta: Arc<Meta>,
    pub(crate) history: StoreHistory<Arc<dyn ObjectStore>>,
    pub(crate) limits: Limits,
    pub(crate) failpoints: Failpoints,
}

impl Repository {
    /// Creates a new repository in `root`.
    pub fn init(root: &Path, opts: &InitOptions) -> Result<Self> {
        let layout = Layout::new(root);
        if layout.config_path().exists() {
            return Err(Error::Conflict(format!("{} is already an AgentGit repository", root.display())));
        }
        std::fs::create_dir_all(layout.ag_dir())?;
        let config =
            RepoConfig { format: 1, namespace: opts.namespace.clone(), default_stream: opts.default_stream.clone() };
        let repo = Self::assemble(layout.clone(), config.clone(), Arc::new(SystemClock))?;
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
        Self::assemble(layout, config, clock)
    }

    /// A throwaway in-memory repository for tests and embedding.
    pub fn ephemeral(namespace: Namespace, default_stream: StreamName, clock: Arc<dyn Clock>) -> Result<Self> {
        let config = RepoConfig { format: 1, namespace, default_stream };
        let objects: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::with_clock(clock.clone()));
        let meta = Arc::new(Meta::open_in_memory(clock)?);
        let limits = Limits::default();
        let repo = Self {
            layout: Layout::new(std::env::temp_dir().join("ag-ephemeral")),
            history: StoreHistory::new(objects.clone(), limits.clone()),
            config: config.clone(),
            objects,
            meta,
            limits,
            failpoints: Failpoints::default(),
        };
        let actor = Identity::new("system:init").expect("valid");
        repo.meta.write(|tx| {
            tx.create_stream(&config.default_stream, actor.as_str())?;
            tx.workspace_create("default", config.default_stream.as_str(), None, actor.as_str())?;
            Ok(())
        })?;
        Ok(repo)
    }

    fn assemble(layout: Layout, config: RepoConfig, clock: Arc<dyn Clock>) -> Result<Self> {
        let limits = Limits::default();
        let objects: Arc<dyn ObjectStore> = Arc::new(FsObjectStore::open(layout.objects_dir(), limits.clone())?);
        let meta = Arc::new(Meta::open_with(&layout.db_path(), clock, limits.clone())?);
        Ok(Self {
            history: StoreHistory::new(objects.clone(), limits.clone()),
            layout,
            config,
            objects,
            meta,
            limits,
            failpoints: Failpoints::default(),
        })
    }

    /// Opens a second handle on the same on-disk repository (own SQLite
    /// connection), e.g. for a worker thread.
    pub fn reopen(&self) -> Result<Self> {
        Self::open_with_clock(self.layout.root(), self.meta.clock().clone())
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
        Ok(resolve_tree(&self.history, &MetaTreeCache(&self.meta), id, &self.limits)?)
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
