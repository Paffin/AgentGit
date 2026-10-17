//! Engine operations that exist mainly for network clients: diffs as data,
//! history negotiation, and the administrative stream compare-and-swap.

use std::collections::BTreeSet;

use ag_changeset::Op;
use ag_dag::{Graph, is_ancestor, reachable};
use ag_diff::{diff_hunks, is_binary, split_lines, unified_diff};
use ag_storage::meta::{IntentRow, StreamState};
use ag_tree::{Entry, Tree};
use ag_types::{ChangesetId, Identity};
use serde::Serialize;
use serde_json::json;

use crate::{Error, Repository, Result};

/// Largest number of files a diff report covers.
const MAX_DIFF_FILES: usize = 500;
/// Largest total patch text a diff report carries.
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
/// Largest number of changesets one history negotiation returns.
pub const MAX_NEGOTIATED: usize = 1_000_000;

/// The diff of one file.
#[derive(Clone, Debug, Serialize)]
pub struct FileDiff {
    /// Path.
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: &'static str,
    /// Binary files carry no patch.
    pub binary: bool,
    /// Lines added.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// Unified diff text.
    pub patch: String,
}

fn line_counts(old: &[u8], new: &[u8]) -> (u32, u32) {
    let (a, b) = (split_lines(old), split_lines(new));
    diff_hunks(&a, &b).iter().fold((0, 0), |(add, del), h| (add + h.b_len as u32, del + h.a_len as u32))
}

impl Repository {
    fn entry_bytes(&self, e: &Entry) -> Result<Vec<u8>> {
        self.blob(&e.content)
    }

    /// The per-file diff between two trees (restricted to `paths` when
    /// non-empty). Returns the files and whether the report was truncated.
    pub fn diff_report(
        &self,
        old: &Tree,
        new: &Tree,
        paths: &[String],
        context: usize,
    ) -> Result<(Vec<FileDiff>, bool)> {
        let wanted = |p: &str| {
            paths.is_empty() || paths.iter().any(|w| p == w || p.starts_with(&format!("{}/", w.trim_end_matches('/'))))
        };
        let mut files = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        for op in old.diff(new) {
            let path = op.path().to_string();
            if !wanted(&path) {
                continue;
            }
            if files.len() >= MAX_DIFF_FILES || bytes >= MAX_DIFF_BYTES {
                truncated = true;
                break;
            }
            let (status, ob, nb): (&'static str, Vec<u8>, Vec<u8>) = match &op {
                Op::Delete { path } => ("deleted", self.entry_bytes(old.get(path).expect("existed"))?, Vec::new()),
                Op::Put { path, .. } => {
                    let ne = new.get(path).expect("exists");
                    match old.get(path) {
                        None => ("added", Vec::new(), self.entry_bytes(ne)?),
                        Some(oe) => ("modified", self.entry_bytes(oe)?, self.entry_bytes(ne)?),
                    }
                }
            };
            let binary = is_binary(&ob) || is_binary(&nb);
            let (additions, deletions, patch) = if binary {
                (0, 0, String::new())
            } else {
                let (a, d) = line_counts(&ob, &nb);
                let (on, nn) = match status {
                    "added" => ("/dev/null".to_string(), format!("b/{path}")),
                    "deleted" => (format!("a/{path}"), "/dev/null".to_string()),
                    _ => (format!("a/{path}"), format!("b/{path}")),
                };
                (a, d, unified_diff(&on, &nn, &ob, &nb, context))
            };
            bytes += patch.len();
            files.push(FileDiff { path, status, binary, additions, deletions, patch });
        }
        Ok((files, truncated))
    }

    /// Changesets reachable from `want` but not from `have`, parents before
    /// children (the order a receiver can publish them in).
    pub fn ancestors_between(&self, want: &[ChangesetId], have: &[ChangesetId]) -> Result<Vec<ChangesetId>> {
        let g = self.graph();
        let known: BTreeSet<ChangesetId> = reachable(&g, have.iter().copied())?;
        let mut seen: BTreeSet<ChangesetId> = BTreeSet::new();
        let mut stack: Vec<ChangesetId> = want.iter().copied().filter(|w| !known.contains(w)).collect();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if seen.len() > MAX_NEGOTIATED {
                return Err(Error::Invalid(format!("more than {MAX_NEGOTIATED} changesets requested")));
            }
            for p in g.parents(&id)? {
                if !known.contains(&p) {
                    stack.push(p);
                }
            }
        }
        let mut keyed = Vec::with_capacity(seen.len());
        for id in seen {
            keyed.push((g.generation(&id)?, id));
        }
        keyed.sort();
        Ok(keyed.into_iter().map(|(_, id)| id).collect())
    }

    /// Moves a stream head directly: compare-and-swap on `(head, version)`.
    ///
    /// This **bypasses the integration pipeline** (no merge, no policy
    /// evaluation, no approvals), so it is an administrator's tool for
    /// repairs and migrations. Unless `force` is set the new head must be a
    /// descendant of the current one. It is audited and appears in the reflog.
    pub fn stream_cas(
        &self,
        stream: &str,
        expected: &StreamState,
        new_head: &ChangesetId,
        actor: &Identity,
        reason: &str,
        force: bool,
    ) -> Result<StreamState> {
        let g = self.graph();
        if !force && let Some(current) = expected.head {
            if !self.meta.read(|tx| tx.is_ready(new_head))? {
                return Err(Error::NotFound(format!("changeset {new_head} is not published")));
            }
            if !is_ancestor(&g, &current, new_head)? {
                return Err(Error::Conflict(
                    "the new head does not contain the current head; this is not a fast-forward (use force)".into(),
                ));
            }
        }
        let reason = if reason.is_empty() { "manual compare-and-swap" } else { reason };
        Ok(self.meta.write(|tx| {
            let st = tx.cas_stream(stream, expected, new_head, None, actor.as_str(), reason)?;
            tx.audit_append(
                actor.as_str(),
                "stream.cas",
                stream,
                &json!({"old_head": expected.head, "new_head": new_head, "version": st.version, "force": force, "reason": reason}),
            )?;
            Ok(st)
        })?)
    }

    /// The recorded operation (publication, integration, ...) by id or by the
    /// idempotency key it was created with.
    pub fn operation_row(&self, operation_id: &str, idempotency_key: &str) -> Result<IntentRow> {
        let found = if !operation_id.is_empty() {
            self.meta.read(|tx| tx.intent(operation_id))?
        } else if !idempotency_key.is_empty() {
            self.meta.read(|tx| tx.intent_by_key(idempotency_key))?
        } else {
            return Err(Error::Invalid("give an operation id or an idempotency key".into()));
        };
        found.ok_or_else(|| Error::NotFound("no such operation".into()))
    }
}
