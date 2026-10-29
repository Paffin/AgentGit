//! Engine operations that exist mainly for network clients: diffs as data,
//! history negotiation, and the administrative stream compare-and-swap.

use std::collections::BTreeSet;

use ag_dag::{Graph, is_ancestor, reachable};
use ag_diff::{diff_hunks, is_binary, split_lines, unified_diff_limited};
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
    /// Git mode before the change, empty for an addition.
    pub old_mode: String,
    /// Git mode after the change, empty for a deletion.
    pub new_mode: String,
    /// Patch omitted because the input or output exceeds this report's byte budget.
    pub truncated: bool,
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
            let before = old.get(op.path());
            let after = new.get(op.path());
            let status = if before.is_none() {
                "added"
            } else if after.is_none() {
                "deleted"
            } else {
                "modified"
            };
            let mode = |entry: Option<&Entry>| {
                match entry {
                    None => "",
                    Some(e) if e.file_type == ag_types::FileType::Symlink => "120000",
                    Some(e) if e.executable => "100755",
                    Some(_) => "100644",
                }
                .to_string()
            };
            let (old_mode, new_mode) = (mode(before), mode(after));
            let remaining = MAX_DIFF_BYTES - bytes;
            let same_content = before.zip(after).is_some_and(|(a, b)| a.content == b.content);
            let mut omitted = false;
            let (mut ob, mut nb) = (Vec::new(), Vec::new());
            if !same_content {
                let size = |entry: Option<&Entry>| -> Result<u64> {
                    match entry {
                        Some(e) => {
                            self.blob_len(&e.content)?.ok_or_else(|| Error::NotFound(format!("blob {}", e.content)))
                        }
                        None => Ok(0),
                    }
                };
                // Bound input before allocating bytes or line vectors. Large files can still expose mode changes.
                omitted = size(before)?.saturating_add(size(after)?) > remaining as u64;
                if !omitted {
                    if let Some(e) = before {
                        ob = self.entry_bytes(e)?;
                    }
                    if let Some(e) = after {
                        nb = self.entry_bytes(e)?;
                    }
                }
            }
            let binary = is_binary(&ob) || is_binary(&nb);
            let (additions, deletions, patch) = if binary || omitted || same_content {
                (0, 0, String::new())
            } else {
                let (on, nn) = match status {
                    "added" => ("/dev/null".to_string(), format!("b/{path}")),
                    "deleted" => (format!("a/{path}"), "/dev/null".to_string()),
                    _ => (format!("a/{path}"), format!("b/{path}")),
                };
                match unified_diff_limited(&on, &nn, &ob, &nb, context, remaining) {
                    Some(patch) => {
                        let (a, d) = line_counts(&ob, &nb);
                        (a, d, patch)
                    }
                    None => {
                        omitted = true;
                        (0, 0, String::new())
                    }
                }
            };
            bytes += patch.len();
            truncated |= omitted;
            files.push(FileDiff {
                path,
                status,
                binary,
                additions,
                deletions,
                patch,
                old_mode,
                new_mode,
                truncated: omitted,
            });
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
