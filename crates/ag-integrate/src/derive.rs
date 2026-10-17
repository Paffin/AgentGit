//! Derived indexes: the outbox dispatcher and tree cache.
//!
//! The metadata index and the object store are the sources of truth. Trees
//! are *derived*: they are computed asynchronously from `changeset.published`
//! outbox events, are verified on read, and can be dropped and rebuilt at any
//! time ([`Repository::reindex`]). Readers never depend on them: a missing
//! tree is recomputed from history.

use ag_changeset::ChangesetSource;
use ag_storage::MetaTreeCache;
use ag_tree::resolve_tree;
use ag_types::ChangesetId;
use serde::Serialize;
use serde_json::json;

use crate::{Error, Repository, Result};

/// Result of a derive pass.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DeriveReport {
    /// Events handled successfully.
    pub processed: usize,
    /// Events that failed and stay pending for a retry.
    pub failed: usize,
    /// Events remaining in the outbox.
    pub pending: u64,
}

impl Repository {
    /// Dispatches up to `limit` pending outbox events (at-least-once).
    pub fn derive_pending(&self, limit: usize) -> Result<DeriveReport> {
        let events = self.meta.read(|tx| tx.outbox_pending(limit))?;
        let mut report = DeriveReport::default();
        for ev in events {
            let outcome: Result<()> = match ev.topic.as_str() {
                "changeset.published" => (|| {
                    let id: ChangesetId = serde_json::from_value(ev.payload["id"].clone())?;
                    self.derive_tree(&id)
                })(),
                // Notification-only topics have no in-process consumer.
                _ => Ok(()),
            };
            match outcome {
                Ok(()) => {
                    self.meta.write(|tx| tx.outbox_ack(ev.id))?;
                    report.processed += 1;
                }
                Err(e) => {
                    self.meta.write(|tx| tx.outbox_fail(ev.id, &e.to_string()))?;
                    report.failed += 1;
                }
            }
        }
        report.pending = self.meta.read(|tx| tx.outbox_pending_count())?;
        Ok(report)
    }

    fn derive_tree(&self, id: &ChangesetId) -> Result<()> {
        let tree = resolve_tree(&self.history, &MetaTreeCache(&self.meta), id, &self.limits)?;
        self.meta.write(|tx| tx.put_derived_tree(id, &tree).map(|_| ()))?;
        Ok(())
    }

    /// Drops every derived index and rebuilds the path index and trees from
    /// the object store. Proves the derived data is disposable.
    pub fn reindex(&self) -> Result<usize> {
        self.meta.write(|tx| tx.clear_derived())?;
        let ids = self.meta.read(|tx| tx.all_changeset_ids())?;
        for id in &ids {
            let cs = self.history.changeset(id).map_err(|e| Error::Invalid(format!("{id}: {e}")))?;
            self.meta.write(|tx| tx.rebuild_paths(id, &cs))?;
        }
        for id in &ids {
            self.derive_tree(id)?;
        }
        self.meta.write(|tx| {
            tx.audit_append("system:reindex", "repo.reindex", "derived", &json!({"changesets": ids.len()}))?;
            Ok(())
        })?;
        Ok(ids.len())
    }
}
