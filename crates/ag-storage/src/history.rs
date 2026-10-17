//! Adapters that expose stored data to the history algorithms.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ag_changeset::{Changeset, ChangesetSource, SourceError};
use ag_object::ObjectKind;
use ag_tree::{Tree, TreeCache};
use ag_types::{ChangesetId, Limits};

use crate::meta::Meta;
use crate::object_store::ObjectStore;

const CACHE_CAP: usize = 8192;

/// A [`ChangesetSource`] backed by an [`ObjectStore`]. Decoded changesets are
/// immutable, so they are cached (bounded) to make history walks cheap.
pub struct StoreHistory<S> {
    store: S,
    limits: Limits,
    cache: Mutex<HashMap<ChangesetId, Arc<Changeset>>>,
}

impl<S: ObjectStore> StoreHistory<S> {
    /// Wraps an object store.
    pub fn new(store: S, limits: Limits) -> Self {
        Self { store, limits, cache: Mutex::new(HashMap::new()) }
    }
}

impl<S: ObjectStore> ChangesetSource for StoreHistory<S> {
    fn changeset(&self, id: &ChangesetId) -> Result<Arc<Changeset>, SourceError> {
        if let Some(c) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            return Ok(c.clone());
        }
        let raw = self
            .store
            .get(&id.object_id())
            .map_err(|e| SourceError::Other(e.to_string()))?
            .ok_or(SourceError::NotFound(*id))?;
        if raw.kind != ObjectKind::Changeset {
            return Err(SourceError::Other(format!("object {id} is not a changeset")));
        }
        let cs =
            Arc::new(Changeset::from_raw(&raw, &self.limits).map_err(|e| SourceError::Other(format!("{id}: {e}")))?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(*id, cs.clone());
        Ok(cs)
    }
}

/// A [`TreeCache`] backed by the derived-tree table of [`Meta`]. Any failure
/// is a cache miss: derived data is disposable.
pub struct MetaTreeCache<'a>(pub &'a Meta);

impl TreeCache for MetaTreeCache<'_> {
    fn cached_tree(&self, id: &ChangesetId) -> Option<Tree> {
        self.0.read(|tx| tx.derived_tree(id)).ok().flatten()
    }
}
