use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ag_types::ChangesetId;

use crate::Changeset;

/// Failure to load a changeset from a [`ChangesetSource`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// The changeset does not exist in the source.
    #[error("changeset {0} not found")]
    NotFound(ChangesetId),
    /// Any other failure (I/O, corruption, ...).
    #[error("{0}")]
    Other(String),
}

/// Read access to published changesets, by identifier.
///
/// History algorithms (`ag-dag`, `ag-tree`) are written against this trait
/// so they work identically over the object store, the metadata index or an
/// in-memory test fixture.
pub trait ChangesetSource {
    /// Loads and decodes a changeset.
    fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>, SourceError>;
}

impl<T: ChangesetSource + ?Sized> ChangesetSource for &T {
    fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>, SourceError> {
        (**self).changeset(id)
    }
}

impl<T: ChangesetSource + ?Sized> ChangesetSource for Arc<T> {
    fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>, SourceError> {
        (**self).changeset(id)
    }
}

/// An in-memory [`ChangesetSource`], handy for tests and tooling.
#[derive(Default, Debug)]
pub struct MemoryHistory {
    map: RwLock<HashMap<ChangesetId, Arc<Changeset>>>,
}

impl MemoryHistory {
    /// Creates an empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a changeset and returns its identifier.
    pub fn insert(&self, cs: Changeset) -> ChangesetId {
        let id = cs.id();
        self.map.write().expect("lock").insert(id, Arc::new(cs));
        id
    }
}

impl ChangesetSource for MemoryHistory {
    fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>, SourceError> {
        self.map.read().expect("lock").get(id).cloned().ok_or(SourceError::NotFound(*id))
    }
}
