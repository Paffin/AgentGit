//! Integrity checking (`ag verify`).

use ag_changeset::ChangesetSource;
use ag_storage::ObjectStatus;
use ag_tree::NoCache;
use ag_tree::resolve_tree;
use serde::Serialize;

use crate::{Repository, Result};

/// "No limit" for the listings below (a count that fits the database's signed 64-bit `LIMIT`).
const ALL_ROWS: usize = i64::MAX as usize;

/// One problem found by [`Repository::fsck`].
#[derive(Clone, Debug, Serialize)]
pub struct FsckIssue {
    /// Short category, e.g. `corrupt_object`.
    pub kind: String,
    /// The object or changeset the problem is about, when it is about one (not for a stream or the audit chain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
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
        self.issues.push(FsckIssue { kind: kind.into(), object: None, detail });
    }

    fn object_issue(&mut self, kind: &str, object: impl ToString, detail: String) {
        self.issues.push(FsckIssue { kind: kind.into(), object: Some(object.to_string()), detail });
    }
}

impl Repository {
    /// Verifies the whole repository: every stored object's hash, every
    /// published changeset against its index row, every stream head (and its
    /// cached tree, recomputed from history), that every other root of
    /// reachability (pins, open proposals, workspaces) names a changeset the
    /// object store can produce, the reflog tails, and the audit chain.
    ///
    /// Not covered: cached trees other than the stream heads' (keyframes and
    /// those of workspaces and pins are verified when they are read), and the
    /// rows that merely mention a proposal or an operation (reviews, queue
    /// entries, intents).
    pub fn fsck(&self) -> Result<FsckReport> {
        let mut r = FsckReport::default();

        for info in self.objects.list()? {
            r.objects_checked += 1;
            match self.objects.verify(&info.id)? {
                ObjectStatus::Ok { .. } => {}
                ObjectStatus::Corrupt(why) => r.object_issue("corrupt_object", info.id, format!("{}: {why}", info.id)),
                ObjectStatus::Missing => r.object_issue("missing_object", info.id, info.id.to_string()),
            }
        }

        let ids = self.meta.read(|tx| tx.all_changeset_ids())?;
        for id in &ids {
            r.changesets_checked += 1;
            match self.history.changeset(id) {
                Ok(cs) => {
                    let parents = self.meta.read(|tx| tx.parents(id))?;
                    if parents != cs.parents {
                        r.object_issue(
                            "index_mismatch",
                            id,
                            format!("{id}: indexed parents differ from the changeset object"),
                        );
                    }
                    for b in cs.blobs() {
                        match self.objects.verify(&b.object_id())? {
                            ObjectStatus::Ok { .. } => {}
                            other => r.object_issue("bad_blob", b, format!("{id} references blob {b}: {other:?}")),
                        }
                    }
                    for p in &cs.parents {
                        if !self.meta.read(|tx| tx.is_ready(p))? {
                            r.object_issue("dangling_parent", id, format!("{id} has unpublished parent {p}"));
                        }
                    }
                }
                Err(e) => r.object_issue("unreadable_changeset", id, format!("{id}: {e}")),
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
                            if let Some(cached) = self.meta.read(|tx| tx.derived_tree(&head))?
                                && cached.id() != fresh.id()
                            {
                                r.issue(
                                    "stale_derived_tree",
                                    format!("cached tree of stream '{}' head differs from history", s.name),
                                );
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
                    r.issue(
                        "reflog_mismatch",
                        format!("stream '{}': reflog does not end at the current head/version", s.name),
                    );
                }
            } else if s.state.head.is_some() {
                r.issue("reflog_mismatch", format!("stream '{}' has a head but an empty reflog", s.name));
            }
        }

        r.issues.extend(self.dangling_refs()?);

        let audit = self.meta.read(|tx| tx.audit_verify())?;
        r.audit_entries = audit.entries;
        r.audit_head = audit.head;
        if let Some(seq) = audit.first_bad {
            r.issue("audit_chain_broken", format!("audit entry #{seq} does not match the hash chain"));
        }
        Ok(r)
    }

    /// The references that keep changesets alive and point at something the object store cannot produce: pins
    /// (`dangling_pin`), open proposals (`dangling_proposal_head`) and workspaces (`dangling_workspace_ref`).
    /// Garbage collection starts from these and refuses to run when one is unreadable. They may name changesets
    /// that are stored but not published yet (a proposal waiting for its integration, a workspace snapshot), so the
    /// test is "can the object store produce it", not "is it published". Cheap compared to [`Repository::fsck`]: it
    /// reads one changeset per reference and hashes nothing else.
    pub fn dangling_refs(&self) -> Result<Vec<FsckIssue>> {
        let mut r = FsckReport::default();
        for pin in self.meta.read(|tx| tx.pins())? {
            self.check_root(&mut r, "dangling_pin", &format!("pin '{}'", pin.name), &pin.changeset);
        }
        for p in self.meta.read(|tx| tx.proposals_list(Some("open"), ALL_ROWS))? {
            self.check_root(&mut r, "dangling_proposal_head", &format!("open proposal {}", p.id), &p.head);
        }
        for w in self.meta.read(|tx| tx.workspaces())? {
            for (what, id) in [("head", &w.head), ("base", &w.base)] {
                if let Some(id) = id {
                    self.check_root(&mut r, "dangling_workspace_ref", &format!("workspace '{}' {what}", w.name), id);
                }
            }
        }
        Ok(r.issues)
    }

    /// Records an issue of `kind` unless the object store yields the changeset `id` that `what` points at.
    fn check_root(&self, r: &mut FsckReport, kind: &str, what: &str, id: &ag_types::ChangesetId) {
        if let Err(e) = self.history.changeset(id) {
            r.issue(kind, format!("{what} points at {id}, which cannot be read: {e}"));
        }
    }
}
