//! Exact-owner file drafts; private bytes never become ordinary repository objects here.
use super::{parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};
use ag_integrate::FileDraftInput;
use ag_protocol::v1;
use ag_storage::meta::FileDraftRow;
use ag_types::{ChangesetId, RepoPath, StreamName};

fn stream(value: &str) -> ApiResult<StreamName> {
    StreamName::new(value).map_err(|e| ApiError::invalid(e.to_string()))
}
fn path(value: &str) -> ApiResult<RepoPath> {
    RepoPath::new(value).map_err(|e| ApiError::invalid(e.to_string()))
}
fn draft(row: Option<FileDraftRow>) -> v1::FileDraft {
    let Some(row) = row else { return v1::FileDraft::default() };
    v1::FileDraft {
        present: row.base.is_some(),
        version: row.version,
        stream: row.stream,
        path: row.path,
        base: row.base.map(|id| id.to_string()).unwrap_or_default(),
        original_blob_id: row.original_blob_id.map(|id| id.to_string()).unwrap_or_default(),
        file_type: row.file_type,
        executable: row.executable,
        content: row.content,
        updated_at_ms: row.updated_at.as_millis(),
    }
}
impl App {
    /// Reads only the authenticated actor's saved bytes.
    pub async fn get_file_draft(&self, call: &Call, req: v1::GetFileDraftRequest) -> ApiResult<v1::FileDraft> {
        let (stream, path) = (stream(&req.stream)?, path(&req.path)?);
        let scope = stream.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceWrite, Some(&scope), move |repo, actor| {
            Ok(draft(repo.file_draft(actor, &stream, &path)?))
        })
        .await
    }
    /// Saves an existing-file edit with an immutable base and version comparison.
    pub async fn save_file_draft(&self, call: &Call, req: v1::SaveFileDraftRequest) -> ApiResult<v1::FileDraft> {
        let (stream, path) = (stream(&req.stream)?, path(&req.path)?);
        let base: ChangesetId = parse_id(require(&req.base, "base")?, "draft base")?;
        let scope = stream.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceWrite, Some(&scope), move |repo, actor| {
            Ok(draft(Some(repo.save_file_draft(
                actor,
                FileDraftInput {
                    stream: &stream,
                    path: &path,
                    base: &base,
                    content: &req.content,
                    expected_version: req.expected_version,
                },
            )?)))
        })
        .await
    }
    /// Clears only the inspected private draft.
    pub async fn clear_file_draft(&self, call: &Call, req: v1::ClearFileDraftRequest) -> ApiResult<v1::FileDraft> {
        let (stream, path) = (stream(&req.stream)?, path(&req.path)?);
        let scope = stream.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceWrite, Some(&scope), move |repo, actor| {
            Ok(draft(repo.clear_file_draft(actor, &stream, &path, req.expected_version)?))
        })
        .await
    }
    /// Lists private metadata for one explicitly scoped branch, without content.
    pub async fn list_file_drafts(
        &self,
        call: &Call,
        req: v1::ListFileDraftsRequest,
    ) -> ApiResult<v1::ListFileDraftsResponse> {
        let stream = stream(&req.stream)?;
        let scope = stream.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceWrite, Some(&scope), move |repo, actor| {
            Ok(v1::ListFileDraftsResponse {
                drafts: repo
                    .file_drafts(actor, &stream)?
                    .into_iter()
                    .map(|row| v1::FileDraftSummary {
                        stream: row.stream,
                        path: row.path,
                        base: row.base.map(|id| id.to_string()).unwrap_or_default(),
                        version: row.version,
                        updated_at_ms: row.updated_at.as_millis(),
                    })
                    .collect(),
            })
        })
        .await
    }
}
