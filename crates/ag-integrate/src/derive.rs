//! Derived indexes: the outbox dispatcher and tree cache.
//!
//! The metadata index and the object store are the sources of truth. Trees
//! are *derived*: they are computed from `changeset.published` outbox events,
//! are verified on read, and can be dropped and rebuilt at any time
//! ([`Repository::reindex`]). Readers never depend on them: a missing tree is
//! recomputed from history.
//!
//! There is no background dispatcher. A pass runs inline, after the
//! transaction of every publication and integration has committed
//! ([`Repository::derive_best_effort`]); it cannot fail the operation that
//! triggered it, so its failure is *recorded* instead
//! ([`Repository::derive_status`]): a failed event stays pending and is
//! retried by the next pass, and `reindex` rebuilds everything. The only
//! in-process consumer is tree derivation; the other topics
//! (`stream.updated`) are notifications that nothing here consumes, marked
//! dispatched on the next pass. The external event feed is the audit log.

use ag_changeset::ChangesetSource;
use ag_storage::MetaTreeCache;
use ag_tree::resolve_tree;
use ag_types::ChangesetId;
use serde::Serialize;
use serde_json::json;

use crate::{Error, Repository, Result};

/// Outbox events handled by the pass that follows a publication.
pub(crate) const DERIVE_BATCH_AFTER_PUBLISH: usize = 64;
/// Outbox events handled by the pass that follows an integration.
pub(crate) const DERIVE_BATCH_AFTER_INTEGRATE: usize = 256;
/// Key, in the repository's `meta` table, of the text of the last failed post-commit pass (empty: it succeeded).
const DERIVE_LAST_ERROR_KEY: &str = "derive.last_error";

/// Trees are cached for every changeset whose first-parent depth is a multiple
/// of this interval, plus the tree of every live head. Any other tree is
/// rebuilt from the nearest cached ancestor by replaying fewer than this many
/// operation lists.
pub const KEYFRAME_INTERVAL: u64 = 32;

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

/// How the derive passes are doing.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DeriveStatus {
    /// Outbox events not yet handled.
    pub pending: u64,
    /// Why the last post-commit pass was incomplete; `None` if it was not (or none ran).
    pub last_error: Option<String>,
}

impl Repository {
    /// Runs the derive pass that follows a committed publication or integration, and records how it went.
    ///
    /// The operation has already committed, so nothing here may fail it: caching the tree of the new `head`
    /// (if there is one) and handling up to `batch` pending events are best effort. A failure is not lost: the
    /// text is kept for [`Repository::derive_status`], failed events stay pending for the next pass, and a later
    /// pass that succeeds clears the record.
    pub(crate) fn derive_best_effort(&self, head: Option<&ChangesetId>, batch: usize) {
        let mut problems = Vec::new();
        // The tree of the new head is computed here and stored in the same commit that acknowledges the events.
        let head_tree =
            head.and_then(|head| match resolve_tree(&self.history, &MetaTreeCache(&self.meta), head, &self.limits) {
                Ok(tree) => Some((*head, tree)),
                Err(e) => {
                    problems.push(format!("caching the tree of {head}: {e}"));
                    None
                }
            });
        match self.derive_batch(batch, head_tree.as_ref()) {
            Ok(r) if r.failed == 0 => {}
            Ok(r) => problems.push(format!("{} outbox event(s) failed and stay pending", r.failed)),
            Err(e) => problems.push(format!("derive pass: {e}")),
        }
        let text = problems.join("; ");
        // Write only when the record changes: a healthy repository does no extra write per operation. If the
        // record itself cannot be updated the metadata store is failing, which the next operation reports.
        if self.meta.read(|tx| tx.meta_get(DERIVE_LAST_ERROR_KEY)).ok().flatten().unwrap_or_default() != text {
            let _ = self.meta.write(|tx| tx.meta_set(DERIVE_LAST_ERROR_KEY, &text));
        }
    }

    /// Pending outbox events and the last failure of a post-commit derive pass.
    pub fn derive_status(&self) -> Result<DeriveStatus> {
        let pending = self.meta.read(|tx| tx.outbox_pending_count())?;
        let last_error = self.meta.read(|tx| tx.meta_get(DERIVE_LAST_ERROR_KEY))?.filter(|s| !s.is_empty());
        Ok(DeriveStatus { pending, last_error })
    }

    /// Dispatches up to `limit` pending outbox events (at-least-once).
    pub fn derive_pending(&self, limit: usize) -> Result<DeriveReport> {
        self.derive_batch(limit, None)
    }

    /// Handles up to `limit` pending events, then records all of it - every acknowledgement and failure, the
    /// optional tree to cache (the live head) and the pruning of derived data - in ONE write transaction. Handling
    /// an event can write too (a keyframe's tree), but the bookkeeping that used to cost a commit per event is one
    /// commit per pass. An acknowledgement that does not reach the database leaves the event pending: it is handled
    /// again, which it is built for (at-least-once).
    fn derive_batch(&self, limit: usize, cache: Option<&(ChangesetId, ag_tree::Tree)>) -> Result<DeriveReport> {
        let events = self.meta.read(|tx| tx.outbox_pending(limit))?;
        let mut report = DeriveReport::default();
        let mut acked = Vec::new();
        let mut failed = Vec::new();
        for ev in events {
            let outcome: Result<()> = match ev.topic.as_str() {
                "changeset.published" => (|| {
                    let id: ChangesetId = serde_json::from_value(ev.payload["id"].clone())?;
                    self.derive_tree(&id)
                })(),
                // Notification-only topics (`stream.updated`) have no in-process consumer: they are recorded in
                // the outbox in the integration's transaction and marked dispatched here. Leaving them pending
                // would grow the backlog and, as the pending list is a bounded oldest-first batch, eventually
                // crowd `changeset.published` out of it.
                _ => Ok(()),
            };
            match outcome {
                Ok(()) => {
                    acked.push(ev.id);
                    report.processed += 1;
                }
                Err(e) => {
                    failed.push((ev.id, e.to_string()));
                    report.failed += 1;
                }
            }
        }
        report.pending = self.meta.write(|tx| {
            if let Some((id, tree)) = cache {
                tx.put_derived_tree(id, tree)?;
            }
            for id in &acked {
                tx.outbox_ack(*id)?;
            }
            for (id, error) in &failed {
                tx.outbox_fail(*id, error)?;
            }
            tx.prune_derived(KEYFRAME_INTERVAL)?;
            tx.outbox_pending_count()
        })?;
        Ok(report)
    }

    /// Caches the tree of `id` if it is a keyframe.
    fn derive_tree(&self, id: &ChangesetId) -> Result<()> {
        self.failpoints.hit("derive.tree")?;
        let depth = self
            .meta
            .read(|tx| tx.fp_depth(id))?
            .ok_or_else(|| Error::NotFound(format!("published changeset {id}")))?;
        if depth % KEYFRAME_INTERVAL != 0 {
            return Ok(());
        }
        self.cache_tree(id)
    }

    /// Unconditionally caches the tree of `id` (used for live heads).
    pub(crate) fn cache_tree(&self, id: &ChangesetId) -> Result<()> {
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
        for root in self.meta.read(|tx| tx.live_roots())? {
            if self.meta.read(|tx| tx.is_ready(&root))? {
                self.cache_tree(&root)?;
            }
        }
        self.meta.write(|tx| tx.prune_derived(KEYFRAME_INTERVAL))?;
        self.meta.write(|tx| {
            tx.audit_append("system:reindex", "repo.reindex", "derived", &json!({"changesets": ids.len()}))?;
            Ok(())
        })?;
        Ok(ids.len())
    }
}
