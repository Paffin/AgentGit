//! Garbage collection.
//!
//! Safety rules, in order of importance:
//!
//! 1. **Fail closed.** If any object reachable from a root cannot be read or
//!    verified, nothing is deleted.
//! 2. **Roots.** Stream heads, pins, open proposals, workspaces, and every head
//!    that appeared in a reflog within the retention window.
//! 3. **Grace period.** Unreachable objects younger than the grace period are
//!    kept, as are objects staged by in-flight publications.
//! 4. **No lost races.** Deletion goes through
//!    [`ObjectStore::delete_if_older_than`], which is atomic with respect to
//!    concurrent writes: re-writing an object refreshes its age, so a writer
//!    that just referenced an old unreachable blob cannot lose it.

use std::collections::{BTreeSet, HashSet};

use ag_changeset::ChangesetSource;
use ag_types::{ChangesetId, ObjectId, Timestamp};
use serde::Serialize;
use serde_json::json;

use crate::{Error, Repository, Result};

/// Garbage collection options.
#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Unreachable objects younger than this are kept.
    pub grace_ms: i64,
    /// Heads that appeared in a reflog within this window stay reachable.
    pub reflog_retention_ms: i64,
    /// Report what would be deleted without deleting.
    pub dry_run: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self { grace_ms: 24 * 3600 * 1000, reflog_retention_ms: 90 * 24 * 3600 * 1000, dry_run: false }
    }
}

/// What a GC run found and did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GcReport {
    /// Changesets reachable from the roots.
    pub reachable_changesets: usize,
    /// Blobs reachable from the roots.
    pub reachable_blobs: usize,
    /// Objects in the store.
    pub scanned: usize,
    /// Unreachable objects that were deleted (or would be, in a dry run).
    pub deleted: Vec<String>,
    /// Bytes freed (or that would be).
    pub bytes: u64,
    /// Unreachable objects kept because of the grace period.
    pub kept_young: usize,
    /// Unreachable objects kept because an in-flight publication staged them.
    pub kept_staged: usize,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl Repository {
    /// Collects unreachable objects. See the module documentation for the safety rules.
    pub fn gc(&self, opts: &GcOptions, actor: &str) -> Result<GcReport> {
        let now = self.clock().now();
        let grace_cutoff = Timestamp::from_millis(now.as_millis() - opts.grace_ms);
        let reflog_since = Timestamp::from_millis(now.as_millis() - opts.reflog_retention_ms);

        // 1. Roots.
        let mut roots: Vec<ChangesetId> = self.meta.read(|tx| tx.live_roots())?;
        roots.extend(self.meta.read(|tx| tx.reflog_heads_since(reflog_since))?);
        roots.sort();
        roots.dedup();

        // 2. Mark. Any failure aborts the whole run before anything is deleted.
        let mut reachable: BTreeSet<ObjectId> = BTreeSet::new();
        let mut seen: HashSet<ChangesetId> = HashSet::new();
        let mut stack = roots;
        let mut changesets = 0usize;
        let mut blobs = 0usize;
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let cs = self.history.changeset(&id).map_err(|e| Error::Invalid(format!("garbage collection aborted: reachable changeset {id} is unreadable: {e}")))?;
            reachable.insert(id.object_id());
            changesets += 1;
            for b in cs.blobs() {
                if reachable.insert(b.object_id()) {
                    blobs += 1;
                }
                match self.objects.verify(&b.object_id())? {
                    ag_storage::ObjectStatus::Ok { .. } => {}
                    other => {
                        return Err(Error::Invalid(format!("garbage collection aborted: reachable blob {b} is {other:?}")));
                    }
                }
            }
            stack.extend(cs.parents.iter().copied());
        }

        // 3. Sweep.
        let staged: HashSet<ObjectId> = self.meta.read(|tx| tx.protected_intent_objects(grace_cutoff))?.into_iter().collect();
        let mut report = GcReport { reachable_changesets: changesets, reachable_blobs: blobs, dry_run: opts.dry_run, ..GcReport::default() };
        let mut deleted_changesets: Vec<ChangesetId> = Vec::new();
        for info in self.objects.list()? {
            report.scanned += 1;
            if reachable.contains(&info.id) {
                continue;
            }
            if staged.contains(&info.id) {
                report.kept_staged += 1;
                continue;
            }
            if info.modified >= grace_cutoff {
                report.kept_young += 1;
                continue;
            }
            let removed = if opts.dry_run { true } else { self.objects.delete_if_older_than(&info.id, grace_cutoff)? };
            if removed {
                report.bytes += info.size;
                report.deleted.push(info.id.to_hex());
                deleted_changesets.push(ChangesetId::new(info.id));
            }
        }

        // 4. Forget index rows of deleted changesets (children before parents).
        if !opts.dry_run {
            self.meta.write(|tx| {
                let mut doomed: Vec<(u64, ChangesetId)> = Vec::new();
                for id in &deleted_changesets {
                    if let Some(g) = tx.generation(id)? {
                        doomed.push((g, *id));
                    }
                }
                doomed.sort_by(|a, b| b.cmp(a));
                let ids: Vec<ChangesetId> = doomed.into_iter().map(|(_, i)| i).collect();
                tx.delete_changesets(&ids)?;
                tx.prune_orphan_trees()?;
                tx.audit_append(actor, "gc.run", "store", &json!({"deleted": report.deleted.len(), "bytes": report.bytes, "reachable_changesets": changesets}))?;
                Ok(())
            })?;
        }
        Ok(report)
    }
}
