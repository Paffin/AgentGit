//! Integrity checking (`ag verify`).

use ag_changeset::ChangesetSource;
use ag_storage::ObjectStatus;
use ag_tree::resolve_tree;
use ag_tree::NoCache;
use serde::Serialize;

use crate::{Repository, Result};

/// One problem found by [`Repository::fsck`].
#[derive(Clone, Debug, Serialize)]
pub struct FsckIssue {
    /// Short category, e.g. `corrupt_object`.
    pub kind: String,
    /// What is wrong.
    pub detail: String,
}

/// Result of an integrity check.
#[derive(Clone, Debug, Default, Serialize)]
pub struct FsckReport {
    /// Objects hashed and verified.
    pub objects_checked: usize,
    /// Published changesets cross-checked against the object store.
    pub changesets_checked: usize,
    /// Streams whose head tree was recomputed from scratch.
    pub heads_recomputed: usize,
    /// Audit entries in the chain.
    pub audit_entries: u64,
    /// Hash of the audit chain head; record it externally to detect rollbacks.
    pub audit_head: String,
    /// Problems found. Empty means healthy.
    pub issues: Vec<FsckIssue>,
}

impl FsckReport {
    /// True if nothing is wrong.
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    fn issue(&mut self, kind: &str, detail: String) {
        self.issues.push(FsckIssue { kind: kind.into(), detail });
    }
}

impl Repository {
    /// Verifies the whole repository: every stored object's hash, every
    /// published changeset against its index row, every stream head, the
    /// derived tree caches (recomputed from history), and the audit chain.
    pub fn fsck(&self) -> Result<FsckReport> {
        let mut r = FsckReport::default();

        for info in self.objects.list()? {
            r.objects_checked += 1;
            match self.objects.verify(&info.id)? {
                ObjectStatus::Ok { .. } => {}
                ObjectStatus::Corrupt(why) => r.issue("corrupt_object", format!("{}: {why}", info.id)),
                ObjectStatus::Missing => r.issue("missing_object", info.id.to_string()),
            }
        }

        let ids = self.meta.read(|tx| tx.all_changeset_ids())?;
        for id in &ids {
            r.changesets_checked += 1;
            match self.history.changeset(id) {
                Ok(cs) => {
                    let parents = self.meta.read(|tx| tx.parents(id))?;
                    if parents != cs.parents {
                        r.issue("index_mismatch", format!("{id}: indexed parents differ from the changeset object"));
                    }
                    for b in cs.blobs() {
                        match self.objects.verify(&b.object_id())? {
                            ObjectStatus::Ok { .. } => {}
                            other => r.issue("bad_blob", format!("{id} references blob {b}: {other:?}")),
                        }
                    }
                    for p in &cs.parents {
                        if !self.meta.read(|tx| tx.is_ready(p))? {
                            r.issue("dangling_parent", format!("{id} has unpublished parent {p}"));
                        }
                    }
                }
                Err(e) => r.issue("unreadable_changeset", format!("{id}: {e}")),
            }
        }

        for s in self.meta.read(|tx| tx.list_streams())? {
            if let Some(head) = s.state.head {
                if !self.meta.read(|tx| tx.is_ready(&head))? {
                    r.issue("stream_head_unpublished", format!("stream '{}' points at unpublished {head}", s.name));
                } else {
                    match resolve_tree(&self.history, &NoCache, &head, &self.limits) {
                        Ok(fresh) => {
                            r.heads_recomputed += 1;
                            if let Some(cached) = self.meta.read(|tx| tx.derived_tree(&head))? {
                                if cached.id() != fresh.id() {
                                    r.issue("stale_derived_tree", format!("cached tree of stream '{}' head differs from history", s.name));
                                }
                            }
                        }
                        Err(e) => r.issue("history_not_derivable", format!("stream '{}': {e}", s.name)),
                    }
                }
            }
            // The reflog must end at the current head.
            let log = self.meta.read(|tx| tx.reflog(s.name.as_str(), 1))?;
            if let Some(last) = log.first() {
                if last.new_head != s.state.head || last.version != s.state.version {
                    r.issue("reflog_mismatch", format!("stream '{}': reflog does not end at the current head/version", s.name));
                }
            } else if s.state.head.is_some() {
                r.issue("reflog_mismatch", format!("stream '{}' has a head but an empty reflog", s.name));
            }
        }

        let audit = self.meta.read(|tx| tx.audit_verify())?;
        r.audit_entries = audit.entries;
        r.audit_head = audit.head;
        if let Some(seq) = audit.first_bad {
            r.issue("audit_chain_broken", format!("audit entry #{seq} does not match the hash chain"));
        }
        Ok(r)
    }
}
