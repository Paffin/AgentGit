//! Trees: paths, directories, whole-tree manifests, diffs and file content.

use ag_protocol::v1;
use ag_tree::{DirEntry, Entry, Tree};
use ag_types::RepoPath;

use super::{paginate, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

fn entry_to_wire(path: &RepoPath, e: &Entry, size: u64) -> v1::TreeEntry {
    v1::TreeEntry {
        path: path.to_string(),
        blob_id: e.content.to_string(),
        size,
        file_type: e.file_type.to_string(),
        executable: e.executable,
    }
}

pub(crate) fn tree_page(tree: &Tree, size: u32, token: &str) -> ApiResult<v1::GetTreeResponse> {
    let entries: Vec<v1::TreeEntry> = tree.iter().map(|(p, e)| entry_to_wire(p, e, 0)).collect();
    let total = entries.len() as u64;
    let (page, next) = paginate("tree", entries, size, token)?;
    Ok(v1::GetTreeResponse {
        tree_id: tree.id().to_string(),
        entries: page,
        next_page_token: next,
        total_entries: total,
    })
}

impl App {
    /// What exists at a path.
    pub async fn resolve_path(&self, call: &Call, req: v1::ResolvePathRequest) -> ApiResult<v1::ResolvePathResponse> {
        let rev = if req.rev.is_empty() { "@".to_string() } else { req.rev.clone() };
        let path = req.path.trim_matches('/').to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |repo, _| {
            let tree = repo.tree_of(&repo.resolve_rev(&rev)?)?;
            if path.is_empty() {
                return Ok(v1::ResolvePathResponse { kind: "dir".into(), entry: None });
            }
            let rp = RepoPath::new(&path).map_err(|e| ApiError::invalid(e.to_string()))?;
            if let Some(e) = tree.get(&rp) {
                let size = repo.blob(&e.content).map(|b| b.len() as u64).unwrap_or(0);
                return Ok(v1::ResolvePathResponse {
                    kind: e.file_type.to_string(),
                    entry: Some(entry_to_wire(&rp, e, size)),
                });
            }
            let prefix = format!("{path}/");
            let is_dir = tree.iter().any(|(p, _)| p.as_str().starts_with(&prefix));
            Ok(v1::ResolvePathResponse { kind: if is_dir { "dir" } else { "missing" }.into(), entry: None })
        })
        .await
    }

    /// Lists a directory.
    pub async fn list_directory(
        &self,
        call: &Call,
        req: v1::ListDirectoryRequest,
    ) -> ApiResult<v1::ListDirectoryResponse> {
        let rev = if req.rev.is_empty() { "@".to_string() } else { req.rev.clone() };
        let dir = req.path.trim_matches('/').to_string();
        let (size, token) = (req.page_size, req.page_token.clone());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |repo, _| {
            let tree = repo.tree_of(&repo.resolve_rev(&rev)?)?;
            let items = tree.list_dir(&dir);
            if items.is_empty() && !dir.is_empty() {
                let prefix = format!("{dir}/");
                if !tree.iter().any(|(p, _)| p.as_str().starts_with(&prefix)) {
                    return Err(ApiError::not_found(format!("no such directory '{dir}'")));
                }
            }
            let wire: Vec<v1::DirEntry> = items
                .iter()
                .map(|d| match d {
                    DirEntry::Dir { name } => {
                        v1::DirEntry { name: name.clone(), kind: "dir".into(), ..Default::default() }
                    }
                    DirEntry::File { name, entry } => v1::DirEntry {
                        name: name.clone(),
                        kind: entry.file_type.to_string(),
                        size: 0,
                        blob_id: entry.content.to_string(),
                        executable: entry.executable,
                    },
                })
                .collect();
            let (page, next) = paginate("dir", wire, size, &token)?;
            Ok(v1::ListDirectoryResponse { entries: page, next_page_token: next, tree_id: tree.id().to_string() })
        })
        .await
    }

    /// The whole tree of a revision as a sorted, paginated manifest.
    pub async fn get_tree(&self, call: &Call, req: v1::GetTreeRequest) -> ApiResult<v1::GetTreeResponse> {
        let rev = if req.rev.is_empty() { "@".to_string() } else { req.rev.clone() };
        let (size, token) = (req.page_size, req.page_token.clone());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |repo, _| {
            let tree = repo.tree_of(&repo.resolve_rev(&rev)?)?;
            tree_page(&tree, size, &token)
        })
        .await
    }

    /// Per-file diffs between two revisions (empty `from` = first parent of `to`).
    pub async fn diff(&self, call: &Call, req: v1::DiffRequest) -> ApiResult<v1::DiffResponse> {
        let to = require(&req.to, "to")?.to_string();
        let from = req.from.clone();
        let paths = req.paths.clone();
        let context = if req.context == 0 { 3 } else { req.context.min(100) } as usize;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |repo, _| {
            let to_id = repo.resolve_rev(&to)?;
            let old: Tree = if from.is_empty() {
                let cs = repo.changeset(&to_id)?;
                repo.tree_or_empty(cs.first_parent().as_ref())?
            } else {
                repo.tree_of(&repo.resolve_rev(&from)?)?
            };
            let new = repo.tree_of(&to_id)?;
            let (files, truncated) = repo.diff_report(&old, &new, &paths, context)?;
            Ok(v1::DiffResponse {
                files: files
                    .into_iter()
                    .map(|f| v1::FileDiff {
                        path: f.path,
                        status: f.status.into(),
                        binary: f.binary,
                        additions: f.additions,
                        deletions: f.deletions,
                        patch: f.patch,
                    })
                    .collect(),
                truncated,
            })
        })
        .await
    }

    /// The bytes of a file at a revision.
    pub async fn file_content(&self, call: &Call, req: v1::GetFileRequest) -> ApiResult<Vec<u8>> {
        let rev = if req.rev.is_empty() { "@".to_string() } else { req.rev.clone() };
        let path = require(&req.path, "path")?.trim_matches('/').to_string();
        let out = self
            .with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |repo, _| {
                let tree = repo.tree_of(&repo.resolve_rev(&rev)?)?;
                let rp = RepoPath::new(&path).map_err(|e| ApiError::invalid(e.to_string()))?;
                let e =
                    tree.get(&rp).ok_or_else(|| ApiError::not_found(format!("'{path}' does not exist at {rev}")))?;
                Ok(repo.blob(&e.content)?)
            })
            .await?;
        self.metrics.object_bytes_out.inc_by(out.len() as u64);
        Ok(out)
    }
}
