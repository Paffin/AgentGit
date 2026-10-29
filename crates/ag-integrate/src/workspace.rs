//! Workspaces: an isolated line of work, optionally backed by a directory.
//!
//! A workspace has a **base** (the stream head it last synced with) and a
//! **head** (its latest snapshot). Snapshots are ordinary changesets stored in
//! the object store; they become visible to others only when proposed
//! (published). Syncing brings the stream's head into the workspace:
//!
//! * fast-forward when the workspace has nothing the stream lacks;
//! * a deterministic merge when both moved and the merge is clean;
//! * otherwise the clean parts are applied, conflicting text files are written
//!   with `<<<<<<< ours` markers, and the next snapshot must resolve them.

use std::path::Path;

use ag_dag::{is_ancestor, merge_bases};
use ag_merge::{ConflictKind, TextMerge, merge_text, merge_trees};
use ag_storage::meta::WorkspaceRow;
use ag_tree::Tree;
use ag_types::{ChangesetId, Identity, StreamName};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::repo::RepoBlobs;
use crate::types::ConflictInfo;
use crate::workdir::{IgnoreRules, checkout_tree, scan_against, update_dir};
use crate::{Error, Repository, Result};

/// Result of [`Repository::sync_workspace`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SyncOutcome {
    /// Nothing to do.
    UpToDate,
    /// The workspace moved to the stream head.
    FastForwarded {
        /// New workspace head.
        to: ChangesetId,
    },
    /// A merge changeset was created.
    Merged {
        /// The merge changeset (new workspace head).
        id: ChangesetId,
    },
    /// Conflicts must be resolved in the working directory, then snapshotted.
    Conflicted {
        /// Conflicting paths.
        conflicts: Vec<ConflictInfo>,
    },
}

#[derive(Serialize, Deserialize)]
struct Pending {
    theirs: ChangesetId,
    paths: Vec<String>,
}

fn pending_key(ws: &str) -> String {
    format!("workspace.{ws}.merging")
}

/// One changed path in a working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    /// `added`, `modified`, `deleted`.
    pub kind: &'static str,
    /// The path.
    pub path: String,
}

impl Repository {
    /// Creates a workspace tracking `stream`, starting at the stream's head.
    pub fn workspace_create(&self, name: &str, stream: &StreamName, owner: &Identity) -> Result<WorkspaceRow> {
        self.workspace_create_at(name, stream, owner, None)
    }

    /// Creates a workspace at an explicit revision, validating it before any row is committed.
    pub fn workspace_create_at(
        &self,
        name: &str,
        stream: &StreamName,
        owner: &Identity,
        from: Option<&ChangesetId>,
    ) -> Result<WorkspaceRow> {
        if name.is_empty()
            || name.len() > 64
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Invalid("workspace names use letters, digits, '_' and '-' (max 64)".into()));
        }
        self.meta.write_with_error(|tx| -> Result<WorkspaceRow> {
            let s = tx
                .stream(stream.as_str())?
                .ok_or_else(|| ag_storage::meta::MetaError::NotFound(format!("stream '{stream}'")))?;
            let base = from.or(s.state.head.as_ref());
            if let Some(head) = base {
                self.validate_retention_root(tx, head)?;
            }
            let row = tx.workspace_create(name, stream.as_str(), base, owner.as_str())?;
            tx.audit_append(owner.as_str(), "workspace.create", name, &json!({"stream": stream}))?;
            Ok(row)
        })
    }

    pub(crate) fn move_workspace(
        &self,
        tx: &ag_storage::meta::Tx<'_>,
        expected: &WorkspaceRow,
        base: Option<&ChangesetId>,
        head: Option<&ChangesetId>,
    ) -> Result<WorkspaceRow> {
        tx.workspace_check(expected)?;
        for root in [base, head].into_iter().flatten() {
            self.validate_retention_root(tx, root)?;
        }
        Ok(tx.workspace_compare_and_swap(expected, base, head)?)
    }

    /// Remembers the directory a workspace lives in.
    pub fn workspace_set_dir(&self, name: &str, dir: &Path) -> Result<()> {
        let abs = std::fs::canonicalize(dir)?;
        self.meta.write(|tx| tx.meta_set(&format!("workspace.{name}.dir"), &abs.to_string_lossy()))?;
        Ok(())
    }

    /// The directory recorded for a workspace, if any.
    pub fn workspace_dir(&self, name: &str) -> Result<Option<std::path::PathBuf>> {
        Ok(self
            .meta
            .read(|tx| tx.meta_get(&format!("workspace.{name}.dir")))?
            .filter(|s| !s.is_empty())
            .map(Into::into))
    }

    /// All workspaces.
    pub fn workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        Ok(self.meta.read(|tx| tx.workspaces())?)
    }

    /// Removes a workspace record. Its snapshots stay in the object store until
    /// garbage collection finds them unreachable; its directory is untouched.
    pub fn workspace_remove(&self, name: &str, actor: &Identity) -> Result<()> {
        let observed = self.workspace_row(name)?;
        self.workspace_remove_checked(&observed, actor)
    }

    /// Removes only the workspace row authorized by the caller.
    pub fn workspace_remove_checked(&self, observed: &WorkspaceRow, actor: &Identity) -> Result<()> {
        let name = observed.name.as_str();
        if name == "default" {
            return Err(Error::Conflict("the default workspace cannot be removed".into()));
        }
        let removed = self.meta.write(|tx| {
            tx.workspace_check(observed)?;
            let r = tx.workspace_delete(name)?;
            if r {
                tx.meta_set(&format!("workspace.{name}.dir"), "")?;
                tx.meta_set(&pending_key(name), "")?;
                tx.audit_append(actor.as_str(), "workspace.remove", name, &json!({}))?;
            }
            Ok(r)
        })?;
        if removed { Ok(()) } else { Err(Error::NotFound(format!("workspace '{name}'"))) }
    }

    /// Looks up a workspace.
    pub fn workspace_row(&self, name: &str) -> Result<WorkspaceRow> {
        self.meta.read(|tx| tx.workspace(name))?.ok_or_else(|| Error::NotFound(format!("workspace '{name}'")))
    }

    fn pending_merge(&self, name: &str) -> Result<Option<Pending>> {
        Ok(self
            .meta
            .read(|tx| tx.meta_get(&pending_key(name)))?
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }

    /// Whether a conflicted sync is waiting for resolution.
    pub fn workspace_is_merging(&self, name: &str) -> Result<bool> {
        Ok(self.pending_merge(name)?.is_some())
    }

    /// Changes in `dir` relative to the workspace head.
    pub fn workspace_status(&self, name: &str, dir: &Path) -> Result<Vec<Change>> {
        let ws = self.workspace_row(name)?;
        let rules = IgnoreRules::load(dir)?;
        let head = self.tree_or_empty(ws.head.as_ref())?;
        let (now, _) = scan_against(self, dir, &rules, false, &head)?;
        Ok(head
            .diff(&now)
            .into_iter()
            .map(|op| match &op {
                ag_changeset::Op::Delete { path } => Change { kind: "deleted", path: path.to_string() },
                ag_changeset::Op::Put { path, .. } => {
                    Change { kind: if head.get(path).is_some() { "modified" } else { "added" }, path: path.to_string() }
                }
            })
            .collect())
    }

    /// Records the state of `dir` as the workspace's next snapshot. Returns
    /// `None` if nothing changed. While a conflicted sync is pending, the
    /// snapshot becomes the merge changeset (parents: head + the stream head
    /// that was merged) and refuses to proceed if conflict markers remain.
    pub fn snapshot(
        &self,
        name: &str,
        dir: &Path,
        message: &str,
        author: &Identity,
        force: bool,
    ) -> Result<Option<ChangesetId>> {
        let ws = self.workspace_row(name)?;
        let rules = IgnoreRules::load(dir)?;
        let head_tree = self.tree_or_empty(ws.head.as_ref())?;
        let (tree, _) = scan_against(self, dir, &rules, true, &head_tree)?;
        let pending = self.pending_merge(name)?;
        if tree == head_tree && pending.is_none() {
            return Ok(None);
        }
        if let Some(p) = &pending
            && !force
        {
            let unresolved: Vec<&String> = p
                .paths
                .iter()
                .filter(|path| std::fs::read(dir.join(path)).map(|b| has_markers(&b)).unwrap_or(false))
                .collect();
            if !unresolved.is_empty() {
                return Err(Error::Conflict(format!(
                    "unresolved conflict markers in: {} (resolve them, or pass --force to snapshot anyway)",
                    unresolved.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                )));
            }
        }
        let mut parents: Vec<ChangesetId> = ws.head.into_iter().collect();
        if let Some(p) = &pending {
            parents.push(p.theirs);
        }
        let ops = head_tree.diff(&tree);
        let id = self.commit(parents, author, message, ops)?;
        self.meta.write_with_error(|tx| -> Result<()> {
            self.move_workspace(tx, &ws, ws.base.as_ref(), Some(&id))?;
            if pending.is_some() {
                tx.meta_set(&pending_key(name), "")?;
            }
            tx.audit_append(author.as_str(), "workspace.snapshot", name, &json!({"changeset": id}))?;
            Ok(())
        })?;
        Ok(Some(id))
    }

    /// Writes the workspace head's tree into an empty (or clean) directory.
    pub fn workspace_checkout(&self, name: &str, dir: &Path) -> Result<()> {
        let ws = self.workspace_row(name)?;
        let tree = self.tree_or_empty(ws.head.as_ref())?;
        checkout_tree(self, &tree, dir)
    }

    /// Brings the stream's head into the workspace and updates `dir`.
    /// The directory must have no uncommitted changes.
    pub fn sync_workspace(&self, name: &str, dir: &Path, author: &Identity) -> Result<SyncOutcome> {
        if self.pending_merge(name)?.is_some() {
            return Err(Error::Conflict("a conflicted sync is pending; resolve the files and snapshot first".into()));
        }
        let ws = self.workspace_row(name)?;
        let changes = self.workspace_status(name, dir)?;
        if !changes.is_empty() {
            return Err(Error::Conflict(format!(
                "the directory has {} uncommitted change(s); snapshot them first",
                changes.len()
            )));
        }
        let Some(stream_head) = self.meta.read(|tx| tx.stream(&ws.stream))?.and_then(|s| s.state.head) else {
            return Ok(SyncOutcome::UpToDate);
        };
        let g = self.graph();
        let ours_tree = self.tree_or_empty(ws.head.as_ref())?;
        let theirs_tree = self.tree_of(&stream_head)?;

        let Some(ws_head) = ws.head else {
            self.meta.write_with_error(|tx| -> Result<()> {
                self.move_workspace(tx, &ws, Some(&stream_head), Some(&stream_head))?;
                update_dir(self, &ours_tree, &theirs_tree, dir)
            })?;
            return Ok(SyncOutcome::FastForwarded { to: stream_head });
        };
        if is_ancestor(&g, &stream_head, &ws_head)? {
            self.meta.write_with_error(|tx| self.move_workspace(tx, &ws, Some(&stream_head), Some(&ws_head)))?;
            return Ok(SyncOutcome::UpToDate);
        }
        if is_ancestor(&g, &ws_head, &stream_head)? {
            self.meta.write_with_error(|tx| -> Result<()> {
                self.move_workspace(tx, &ws, Some(&stream_head), Some(&stream_head))?;
                update_dir(self, &ours_tree, &theirs_tree, dir)
            })?;
            return Ok(SyncOutcome::FastForwarded { to: stream_head });
        }
        let bases = merge_bases(&g, &ws_head, &stream_head)?;
        let base = match bases.as_slice() {
            [one] => *one,
            [] => return Err(Error::Invalid("the workspace and the stream have unrelated histories".into())),
            _ => return Err(Error::Invalid("criss-cross history: more than one merge base; resolve manually".into())),
        };
        let base_tree = self.tree_of(&base)?;
        let merged = merge_trees(&base_tree, &ours_tree, &theirs_tree, &RepoBlobs(self), &self.limits)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if merged.is_clean() {
            let ops = ours_tree.diff(&merged.tree);
            let id = self.commit(
                vec![ws_head, stream_head],
                author,
                &format!(
                    "Sync {} into workspace '{name}'\n\nMerge-Base: {base}\nMerge-Algorithm: {}\n{}",
                    ws.stream,
                    ag_merge::ALGORITHM_VERSION,
                    crate::integrate::note_trailers(&merged.notes)
                ),
                ops,
            )?;
            self.meta.write_with_error(|tx| -> Result<()> {
                self.move_workspace(tx, &ws, Some(&stream_head), Some(&id))?;
                update_dir(self, &ours_tree, &merged.tree, dir)?;
                tx.audit_append(author.as_str(), "workspace.sync", name, &json!({"merge": id}))?;
                Ok(())
            })?;
            return Ok(SyncOutcome::Merged { id });
        }

        // Conflicted: apply the clean part, then write marker files for text conflicts.
        self.meta.write_with_error(|tx| -> Result<SyncOutcome> {
            tx.workspace_check(&ws)?;
            update_dir(self, &ours_tree, &merged.tree, dir)?;
            let mut paths = Vec::new();
            let mut infos = Vec::new();
            for c in &merged.conflicts {
                infos.push(ConflictInfo::from_conflict(c));
                paths.push(c.path.to_string());
                if let ConflictKind::Content { .. } = c.kind {
                    // After a rename the three versions live at different paths; the marker file goes where
                    // the merged result keeps the file.
                    let (bp, op, tp) = match &c.origin {
                        Some(o) => (&o.base, &o.ours, &o.theirs),
                        None => (&c.path, &c.path, &c.path),
                    };
                    let (b, o, t) = (base_tree.get(bp), ours_tree.get(op), theirs_tree.get(tp));
                    if let (Some(b), Some(o), Some(t)) = (b, o, t)
                        && let TextMerge::Conflicted { rendered, .. } =
                            merge_text(&self.blob(&b.content)?, &self.blob(&o.content)?, &self.blob(&t.content)?)
                    {
                        let mut abs = dir.to_path_buf();
                        abs.extend(c.path.components());
                        std::fs::write(abs, rendered)?;
                    }
                }
            }
            let pending = Pending { theirs: stream_head, paths };
            self.move_workspace(tx, &ws, Some(&stream_head), Some(&ws_head))?;
            tx.meta_set(&pending_key(name), &serde_json::to_string(&pending).expect("serializes"))?;
            tx.audit_append(author.as_str(), "workspace.sync_conflict", name, &json!({"conflicts": infos.len()}))?;
            Ok(SyncOutcome::Conflicted { conflicts: infos })
        })
    }

    /// Re-targets a workspace (and its directory) at an arbitrary stored changeset.
    pub fn workspace_reset(&self, name: &str, dir: &Path, target: &ChangesetId) -> Result<()> {
        let ws = self.workspace_row(name)?;
        let changes = self.workspace_status(name, dir)?;
        if !changes.is_empty() {
            return Err(Error::Conflict(format!(
                "the directory has {} uncommitted change(s); snapshot or discard them first",
                changes.len()
            )));
        }
        let old = self.tree_or_empty(ws.head.as_ref())?;
        let new: Tree = self.tree_of(target)?;
        self.meta.write_with_error(|tx| -> Result<()> {
            self.move_workspace(tx, &ws, Some(target), Some(target))?;
            update_dir(self, &old, &new, dir)?;
            tx.meta_set(&pending_key(name), "")?;
            Ok(())
        })?;
        Ok(())
    }
}

fn has_markers(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == b'\n').any(|l| l.starts_with(b"<<<<<<< ours") || l.starts_with(b">>>>>>> theirs"))
}
