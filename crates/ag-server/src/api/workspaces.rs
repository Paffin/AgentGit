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
                "" | "regular" => FileType::Regular,
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
fn check_owner(repo: &ag_integrate::Repository, name: &str, actor: &ag_types::Identity, admin: bool) -> ApiResult<()> {
    let ws = repo.workspace_row(name)?;
    if admin
        || ws.owner == actor.to_string()
        || ag_types::Identity::new(ws.owner.clone()).map(|o| o.principal() == actor.principal()).unwrap_or(false)
    {
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
            let row = repo.workspace_create(&name, &stream, actor).map_err(|e| match e {
                ag_integrate::Error::Meta(ag_storage::meta::MetaError::AlreadyExists(m)) => {
                    ApiError::new(ag_protocol::ErrorCode::AlreadyExists, m)
                }
                other => other.into(),
            })?;
            if !from_rev.is_empty() {
                let id = repo.resolve_rev(&from_rev)?;
                repo.meta().write(|tx| tx.workspace_update(&name, Some(&id), Some(&id)))?;
                return Ok(convert::workspace(&repo.workspace_row(&name)?));
            }
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
        self.with_repo_mut(call, &repo_name, Cap::WorkspaceWrite, None, move |repo, actor| {
            check_owner(repo, &name, actor, admin)?;
            let row = repo.workspace_modify(&name, &list, &message, actor, expected.as_ref())?;
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
        let admin = self.is_repo_admin(call, &repo_name).await;
        let name = req.name.clone();
        self.with_repo_mut(call, &repo_name, Cap::WorkspaceWrite, None, move |repo, actor| {
            check_owner(repo, &name, actor, admin)?;
            let outcome = repo.workspace_sync_virtual(&name, actor, &resolutions)?;
            let head = repo.workspace_row(&name)?.head.map(|h| h.to_string()).unwrap_or_default();
            Ok(match outcome {
                SyncOutcome::UpToDate => {
                    v1::SyncWorkspaceResponse { status: "up_to_date".into(), head, conflicts: vec![] }
                }
                SyncOutcome::FastForwarded { .. } => {
                    v1::SyncWorkspaceResponse { status: "fast_forwarded".into(), head, conflicts: vec![] }
                }
                SyncOutcome::Merged { .. } => {
                    v1::SyncWorkspaceResponse { status: "merged".into(), head, conflicts: vec![] }
                }
                SyncOutcome::Conflicted { conflicts } => v1::SyncWorkspaceResponse {
                    status: "conflicted".into(),
                    head,
                    conflicts: conflicts.iter().map(convert::conflict).collect(),
                },
            })
        })
        .await
    }

    /// Removes a workspace record.
    pub async fn delete_workspace(&self, call: &Call, req: v1::DeleteWorkspaceRequest) -> ApiResult<v1::Empty> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let admin = self.is_repo_admin(call, &repo_name).await;
        let name = req.name.clone();
        self.with_repo_mut(call, &repo_name, Cap::WorkspaceWrite, None, move |repo, actor| {
            check_owner(repo, &name, actor, admin)?;
            repo.workspace_remove(&name, actor)?;
            Ok(v1::Empty {})
        })
        .await
    }
}
