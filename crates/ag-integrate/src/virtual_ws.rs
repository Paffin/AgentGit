//! Virtual workspaces: an isolated line of work edited through the API.
//!
//! The same workspace record as for directory workspaces, but no checkout is
//! needed: an agent applies batches of file changes and each batch becomes
//! one new (unpublished) changeset on top of the workspace head. Syncing with
//! the stream is a deterministic merge; conflicts are reported and resolved by
//! supplying the resolved files, which produces a proper two-parent merge
//! changeset.

use std::collections::BTreeMap;

use ag_changeset::Op;
use ag_dag::{is_ancestor, merge_bases};
use ag_merge::merge_trees;
use ag_storage::meta::WorkspaceRow;
use ag_tree::Tree;
use ag_types::{BlobId, ChangesetId, FileType, Identity, RepoPath};
use serde_json::json;

use crate::repo::RepoBlobs;
use crate::types::ConflictInfo;
use crate::workspace::SyncOutcome;
use crate::{Error, Repository, Result};

/// New content of a file.
#[derive(Clone, Debug)]
pub enum VirtualContent {
    /// Inline bytes (stored as a blob).
    Bytes(Vec<u8>),
    /// A blob that was uploaded before.
    Blob(BlobId),
}

/// One change to a virtual workspace.
#[derive(Clone, Debug)]
pub enum VirtualChange {
    /// Create or replace a file (or symlink).
    Put {
        /// Target path.
        path: RepoPath,
        /// Content (for symlinks: the link target).
        content: VirtualContent,
        /// Executable bit.
        executable: bool,
        /// Regular file or symlink.
        file_type: FileType,
    },
    /// Remove a file.
    Delete {
        /// Target path.
        path: RepoPath,
    },
}

impl VirtualChange {
    fn path(&self) -> &RepoPath {
        match self {
            VirtualChange::Put { path, .. } | VirtualChange::Delete { path } => path,
        }
    }
}

impl Repository {
    fn change_to_target(&self, c: &VirtualChange) -> Result<Option<ag_tree::Entry>> {
        match c {
            VirtualChange::Delete { .. } => Ok(None),
            VirtualChange::Put { content, executable, file_type, .. } => {
                let blob = match content {
                    VirtualContent::Bytes(b) => self.put_blob(b.clone())?,
                    VirtualContent::Blob(id) => {
                        if !self.objects.contains(&id.object_id())? {
                            return Err(Error::NotFound(format!("blob {id} has not been uploaded")));
                        }
                        *id
                    }
                };
                Ok(Some(ag_tree::Entry {
                    content: blob,
                    file_type: *file_type,
                    executable: *executable && *file_type == FileType::Regular,
                }))
            }
        }
    }

    /// Turns a set of desired path states into the minimal ops against `tree`.
    fn ops_towards(&self, tree: &Tree, wanted: &BTreeMap<RepoPath, Option<ag_tree::Entry>>) -> Vec<Op> {
        let mut ops = Vec::new();
        for (path, want) in wanted {
            match (tree.get(path), want) {
                (None, None) => {}
                (Some(_), None) => ops.push(Op::Delete { path: path.clone() }),
                (cur, Some(e)) => {
                    if cur != Some(e) {
                        ops.push(Op::Put {
                            path: path.clone(),
                            content: e.content,
                            file_type: e.file_type,
                            executable: e.executable,
                        });
                    }
                }
            }
        }
        ops
    }

    /// Applies `changes` to the workspace as ONE new changeset. Later changes
    /// to the same path win. Returns the updated workspace (unchanged if the
    /// batch is a no-op). `expected_head` makes the call optimistic: it is
    /// refused if the workspace moved.
    pub fn workspace_modify(
        &self,
        name: &str,
        changes: &[VirtualChange],
        message: &str,
        author: &Identity,
        expected_head: Option<&ChangesetId>,
    ) -> Result<WorkspaceRow> {
        if self.workspace_is_merging(name)? {
            return Err(Error::Conflict("this workspace has a conflicted directory sync pending".into()));
        }
        let ws = self.workspace_row(name)?;
        if let Some(expected) = expected_head
            && ws.head.as_ref() != Some(expected)
        {
            return Err(Error::Conflict(format!(
                "workspace '{name}' moved: expected head {expected}, found {}",
                ws.head.map(|h| h.to_string()).unwrap_or_else(|| "(empty)".into())
            )));
        }
        if changes.is_empty() {
            return Err(Error::Invalid("no changes given".into()));
        }
        let mut wanted: BTreeMap<RepoPath, Option<ag_tree::Entry>> = BTreeMap::new();
        for c in changes {
            wanted.insert(c.path().clone(), self.change_to_target(c)?);
        }
        let head_tree = self.tree_or_empty(ws.head.as_ref())?;
        let ops = self.ops_towards(&head_tree, &wanted);
        if ops.is_empty() {
            return self.with_unchanged_workspace(&ws, |tx| {
                tx.workspace(name)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound(format!("workspace '{name}'")))
            });
        }
        // Validate (file/directory clashes, limits) before creating anything.
        head_tree.apply(&ops, &self.limits)?;
        let id = self.commit(ws.head.into_iter().collect(), author, message, ops)?;
        self.with_unchanged_workspace(&ws, |tx| {
            tx.workspace_update(name, ws.base.as_ref(), Some(&id))?;
            tx.audit_append(author.as_str(), "workspace.modify", name, &json!({"changeset": id}))?;
            tx.workspace(name)?.ok_or_else(|| ag_storage::meta::MetaError::Corrupt("workspace vanished".into()))
        })
    }

    /// Brings the stream head into a virtual workspace.
    ///
    /// Without `resolutions`, a conflicting merge changes nothing and returns
    /// [`SyncOutcome::Conflicted`]. With `resolutions` covering **every**
    /// conflicting path, the clean parts of the merge plus the supplied files
    /// become a two-parent merge changeset.
    pub fn workspace_sync_virtual(
        &self,
        name: &str,
        author: &Identity,
        resolutions: &[VirtualChange],
    ) -> Result<SyncOutcome> {
        let ws = self.workspace_row(name)?;
        let Some(stream_head) = self.meta.read(|tx| tx.stream(&ws.stream))?.and_then(|s| s.state.head) else {
            return Ok(SyncOutcome::UpToDate);
        };
        let g = self.graph();
        let Some(ws_head) = ws.head else {
            self.with_unchanged_workspace(&ws, |tx| tx.workspace_update(name, Some(&stream_head), Some(&stream_head)))?;
            return Ok(SyncOutcome::FastForwarded { to: stream_head });
        };
        if is_ancestor(&g, &stream_head, &ws_head)? {
            self.with_unchanged_workspace(&ws, |tx| tx.workspace_update(name, Some(&stream_head), Some(&ws_head)))?;
            return Ok(SyncOutcome::UpToDate);
        }
        if is_ancestor(&g, &ws_head, &stream_head)? {
            self.with_unchanged_workspace(&ws, |tx| tx.workspace_update(name, Some(&stream_head), Some(&stream_head)))?;
            return Ok(SyncOutcome::FastForwarded { to: stream_head });
        }
        let base = match merge_bases(&g, &ws_head, &stream_head)?.as_slice() {
            [one] => *one,
            [] => return Err(Error::Invalid("the workspace and the stream have unrelated histories".into())),
            _ => return Err(Error::Invalid("criss-cross history: more than one merge base; resolve manually".into())),
        };
        let ours = self.tree_of(&ws_head)?;
        let theirs = self.tree_of(&stream_head)?;
        let base_tree = self.tree_of(&base)?;
        let merged = merge_trees(&base_tree, &ours, &theirs, &RepoBlobs(self), &self.limits)
            .map_err(|e| Error::Invalid(e.to_string()))?;

        let mut result = merged.tree.clone();
        if !merged.is_clean() {
            let infos: Vec<ConflictInfo> = merged.conflicts.iter().map(ConflictInfo::from_conflict).collect();
            let mut wanted: BTreeMap<RepoPath, Option<ag_tree::Entry>> = BTreeMap::new();
            for r in resolutions {
                wanted.insert(r.path().clone(), self.change_to_target(r)?);
            }
            let unresolved: Vec<&ConflictInfo> =
                infos.iter().filter(|c| !wanted.keys().any(|p| p.as_str() == c.path)).collect();
            if resolutions.is_empty() || !unresolved.is_empty() {
                return Ok(SyncOutcome::Conflicted { conflicts: infos });
            }
            let ops = self.ops_towards(&result, &wanted);
            result = result.apply(&ops, &self.limits)?;
        }
        let ops = ours.diff(&result);
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
        self.with_unchanged_workspace(&ws, |tx| {
            tx.workspace_update(name, Some(&stream_head), Some(&id))?;
            tx.audit_append(author.as_str(), "workspace.sync", name, &json!({"merge": id}))?;
            Ok(())
        })?;
        Ok(SyncOutcome::Merged { id })
    }
}
