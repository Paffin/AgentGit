//! Virtual workspaces: isolated lines of work edited through the API.

use ag_integrate::{SyncOutcome, VirtualChange, VirtualContent};
use ag_protocol::v1;
use ag_types::{BlobId, ChangesetId, FileType, RepoPath, StreamName};

use super::{convert, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

fn to_change(c: &v1::FileChange) -> ApiResult<VirtualChange> {
    let path = RepoPath::new(&c.path).map_err(|e| ApiError::invalid(format!("path '{}': {e}", c.path)))?;
    match c.op.as_str() {
        "delete" => Ok(VirtualChange::Delete { path }),
        "put" | "" => {
            let file_type = match c.file_type.as_str() {
                // "file" is what listings used to say; keep accepting it.
                "" | "regular" | "file" => FileType::Regular,
                "symlink" => FileType::Symlink,
                other => return Err(ApiError::invalid(format!("unknown file_type '{other}'"))),
            };
            let content = if !c.blob_id.is_empty() {
                VirtualContent::Blob(parse_id::<BlobId>(&c.blob_id, "blob id")?)
            } else {
                VirtualContent::Bytes(c.content.clone())
            };
            Ok(VirtualChange::Put { path, content, executable: c.executable, file_type })
        }
        other => Err(ApiError::invalid(format!("unknown op '{other}' (expected put or delete)"))),
    }
}

fn changes(list: &[v1::FileChange]) -> ApiResult<Vec<VirtualChange>> {
    if list.len() > ag_protocol::limits::MAX_BATCH as usize {
        return Err(ApiError::new(ag_protocol::ErrorCode::TooLarge, "too many changes in one request"));
    }
    list.iter().map(to_change).collect()
}

/// Workspaces are private to their owner unless the caller administers the repository.
fn check_owner(ws: &ag_storage::meta::WorkspaceRow, actor: &ag_types::Identity, admin: bool) -> ApiResult<()> {
    let name = &ws.name;
    if admin || super::owns(&ws.owner, actor) {
        Ok(())
    } else {
        Err(ApiError::denied(format!("workspace '{name}' belongs to {}", ws.owner)))
    }
}

impl App {
    async fn is_repo_admin(&self, call: &Call, repo: &str) -> bool {
        self.authorize(call, repo, Cap::RepoConfigure, None).await.is_ok()
    }

    /// Creates a workspace.
    pub async fn create_workspace(&self, call: &Call, req: v1::CreateWorkspaceRequest) -> ApiResult<v1::Workspace> {
        let stream = StreamName::new(&req.stream).map_err(|e| ApiError::invalid(e.to_string()))?;
        let (name, from_rev) = (require(&req.name, "name")?.to_string(), req.from_rev.clone());
        let scope = stream.to_string();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::WorkspaceWrite, Some(&scope), move |repo, actor| {
            let from = if from_rev.is_empty() { None } else { Some(repo.resolve_rev(&from_rev)?) };
            let row = repo.workspace_create_at(&name, &stream, actor, from.as_ref()).map_err(|e| match e {
                ag_integrate::Error::Meta(ag_storage::meta::MetaError::AlreadyExists(m)) => {
                    ApiError::new(ag_protocol::ErrorCode::AlreadyExists, m)
                }
                other => other.into(),
            })?;
            Ok(convert::workspace(&row))
        })
        .await
    }

    /// One workspace.
    pub async fn get_workspace(&self, call: &Call, req: v1::GetWorkspaceRequest) -> ApiResult<v1::Workspace> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceRead, None, move |repo, _| {
            Ok(convert::workspace(&repo.workspace_row(&req.name)?))
        })
        .await
    }

    /// All workspaces.
    pub async fn list_workspaces(
        &self,
        call: &Call,
        req: v1::ListWorkspacesRequest,
    ) -> ApiResult<v1::ListWorkspacesResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::WorkspaceRead, None, |repo, _| {
            Ok(v1::ListWorkspacesResponse { workspaces: repo.workspaces()?.iter().map(convert::workspace).collect() })
        })
        .await
    }

    /// Applies a batch of file changes as one new changeset.
    pub async fn modify_workspace(&self, call: &Call, req: v1::ModifyWorkspaceRequest) -> ApiResult<v1::Workspace> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let list = changes(&req.changes)?;
        let expected: Option<ChangesetId> =
            if req.expected_head.is_empty() { None } else { Some(parse_id(&req.expected_head, "changeset id")?) };
        let message = if req.message.is_empty() { "Update workspace".to_string() } else { req.message.clone() };
        let admin = self.is_repo_admin(call, &repo_name).await;
        let name = req.name.clone();
        self.with_workspace_mut(call, &repo_name, Cap::WorkspaceWrite, name.clone(), move |repo, actor, observed| {
            check_owner(observed, actor, admin)?;
            let row = repo.workspace_modify_checked(observed, &list, &message, actor, expected.as_ref())?;
            Ok(convert::workspace(&row))
        })
        .await
    }

    /// Brings the stream head into the workspace.
    pub async fn sync_workspace(
        &self,
        call: &Call,
        req: v1::SyncWorkspaceRequest,
    ) -> ApiResult<v1::SyncWorkspaceResponse> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let resolutions = changes(&req.resolutions)?;
        let expected = if req.expected_stream_head.is_empty() {
            None
        } else {
            Some(parse_id::<ChangesetId>(&req.expected_stream_head, "inspected branch head")?)
        };
        let admin = self.is_repo_admin(call, &repo_name).await;
        let name = req.name.clone();
        self.with_workspace_mut(call, &repo_name, Cap::WorkspaceWrite, name, move |repo, actor, observed| {
            check_owner(observed, actor, admin)?;
            let current = repo.meta().read(|tx| tx.stream(&observed.stream))?.and_then(|s| s.state.head);
            let target = expected.or(current);
            let outcome = repo.workspace_sync_virtual_checked_at(observed, actor, &resolutions, target.as_ref())?;
            let head = match &outcome {
                SyncOutcome::FastForwarded { to } => Some(*to),
                SyncOutcome::Merged { id } => Some(*id),
                _ => observed.head,
            }
            .map(|head| head.to_string())
            .unwrap_or_default();
            let (status, conflicts) = match outcome {
                SyncOutcome::UpToDate => ("up_to_date", vec![]),
                SyncOutcome::FastForwarded { .. } => ("fast_forwarded", vec![]),
                SyncOutcome::Merged { .. } => ("merged", vec![]),
                SyncOutcome::Conflicted { conflicts } => {
                    ("conflicted", conflicts.iter().map(convert::conflict).collect())
                }
            };
            Ok(v1::SyncWorkspaceResponse {
                status: status.into(),
                head,
                conflicts,
                stream_head: target.map(|head| head.to_string()).unwrap_or_default(),
            })
        })
        .await
    }

    /// Removes a workspace record.
    pub async fn delete_workspace(&self, call: &Call, req: v1::DeleteWorkspaceRequest) -> ApiResult<v1::Empty> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let admin = self.is_repo_admin(call, &repo_name).await;
        let name = req.name.clone();
        self.with_workspace_mut(call, &repo_name, Cap::WorkspaceWrite, name.clone(), move |repo, actor, observed| {
            check_owner(observed, actor, admin)?;
            repo.workspace_remove_checked(observed, actor)?;
            Ok(v1::Empty {})
        })
        .await
    }
}
