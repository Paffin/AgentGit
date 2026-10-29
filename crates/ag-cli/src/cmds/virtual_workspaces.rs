//! Explicit server-side workspace operations; no checkout or automatic conflict choice.
use std::io::Read;
use std::path::{Path, PathBuf};

use ag_client::{Client, ClientError};
use ag_protocol::{features, v1};
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

/// Server-side workspace lifecycle.
#[derive(Subcommand, Debug)]
pub enum WorkspaceCmd {
    /// List server-side workspaces.
    List,
    /// Show a server-side workspace.
    Show {
        /// Server-side workspace name.
        name: String,
    },
    /// Create an isolated workspace, optionally at an existing revision.
    Create {
        /// New workspace name.
        name: String,
        /// Stream to follow.
        #[arg(long, default_value = "main")]
        stream: String,
        /// Starting revision instead of the stream head.
        #[arg(long, value_name = "REVISION")]
        from: Option<String>,
    },
    /// Merge the stream into a workspace; conflicts leave it unchanged.
    Sync(SyncArgs),
    /// Delete the named server-side workspace.
    Delete {
        /// Server-side workspace name.
        name: String,
    },
}

/// Optional explicit resolutions and their inspected stream revision.
#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Server-side workspace name.
    name: String,
    /// Explicit FileChange JSON array; content is base64, or use an existing blob_id.
    #[arg(long, value_name = "PATH", requires = "expected_stream_head")]
    resolutions: Option<PathBuf>,
    /// Exact nonempty stream head inspected in the preceding sync result.
    #[arg(long, value_name = "CHANGESET")]
    expected_stream_head: Option<String>,
}

impl WorkspaceCmd {
    /// Whether this invocation may change a server-side workspace.
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::List | Self::Show { .. })
    }
}

fn require_features(cmd: &WorkspaceCmd, supported: &[String]) -> Result<()> {
    let mut needed = vec![features::WORKSPACES_VIRTUAL];
    if let WorkspaceCmd::Sync(args) = cmd {
        if args.expected_stream_head.is_some() {
            needed.push(features::WORKSPACES_SYNC_PRECONDITION);
        }
        if args.resolutions.is_some() {
            needed.push(features::WORKSPACES_STRUCTURAL_RESOLUTION);
        }
    }
    for feature in needed {
        if !supported.iter().any(|value| value == feature) {
            bail!("server does not support {feature}; update it before using this command");
        }
    }
    Ok(())
}

fn parse_resolution(value: Value) -> Result<v1::FileChange> {
    let object = value.as_object().context("each resolution must be an object")?;
    let allowed = ["path", "op", "content", "blob_id", "executable", "file_type"];
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("unknown resolution field {key}");
    }
    let change: v1::FileChange = serde_json::from_value(value.clone()).context("invalid resolution")?;
    ag_types::RepoPath::new(&change.path)?;
    validate_resolution_content(object, &change)?;
    Ok(change)
}

fn validate_resolution_content(object: &serde_json::Map<String, Value>, change: &v1::FileChange) -> Result<()> {
    match change.op.as_str() {
        "put" => {
            if object.contains_key("content") == !change.blob_id.is_empty() {
                bail!("put requires exactly one of base64 content or blob_id");
            }
        }
        "delete" => {
            if ["content", "blob_id", "executable", "file_type"].iter().any(|key| object.contains_key(*key)) {
                bail!("delete accepts only path and op");
            }
        }
        _ => bail!("resolution op must be put or delete"),
    }
    Ok(())
}

fn resolutions(path: Option<&Path>) -> Result<Vec<v1::FileChange>> {
    let Some(path) = path else { return Ok(Vec::new()) };
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("resolution file exceeds 8 MiB");
    }
    let values: Vec<Value> = serde_json::from_slice(&bytes).context("resolutions must be a JSON array")?;
    if values.is_empty() || values.len() > ag_protocol::limits::MAX_BATCH as usize {
        bail!("resolution array must contain 1 to {} entries", ag_protocol::limits::MAX_BATCH);
    }
    let changes = values.into_iter().map(parse_resolution).collect::<Result<Vec<_>>>()?;
    require_unique_paths(&changes)?;
    Ok(changes)
}

fn require_unique_paths(changes: &[v1::FileChange]) -> Result<()> {
    let mut paths = std::collections::BTreeSet::new();
    if changes.iter().any(|change| !paths.insert(change.path.as_str())) {
        bail!("a resolution path appears more than once");
    }
    Ok(())
}

fn describe_workspace(workspace: &v1::Workspace) -> String {
    format!(
        "{} -> {}\nhead: {}\nbase: {}\nowner: {}",
        workspace.name, workspace.stream, workspace.head, workspace.base, workspace.owner
    )
}

fn describe_sync(result: &v1::SyncWorkspaceResponse) -> String {
    let mut message =
        format!("{}\nworkspace head: {}\nstream head: {}", result.status, result.head, result.stream_head);
    for conflict in &result.conflicts {
        message.push_str(&format!("\nconflict: {}", super::conflicts::wire(conflict)));
        if let Some(scope) = &conflict.resolution_scope {
            message.push_str(&format!(
                "\n    resolution scope ({}): {}",
                if scope.complete { "complete" } else { "incomplete" },
                scope.paths.join(", ")
            ));
        }
    }
    message
}

async fn sync(client: &Client, repo: &str, args: &SyncArgs) -> Result<(Value, String, bool)> {
    if let Some(head) = &args.expected_stream_head {
        head.parse::<ag_types::ChangesetId>().context("expected stream head must be a full changeset id")?;
    }
    let result = client
        .workspaces()
        .sync(client.req(v1::SyncWorkspaceRequest {
            repo: repo.into(),
            name: args.name.clone(),
            resolutions: resolutions(args.resolutions.as_deref())?,
            expected_stream_head: args.expected_stream_head.clone().unwrap_or_default(),
            ..Default::default()
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let message = describe_sync(&result);
    let conflicted = result.status == "conflicted";
    Ok((serde_json::to_value(result)?, message, conflicted))
}

/// Executes only the operation explicitly named by the caller.
pub async fn run(client: &Client, repo: &str, cmd: &WorkspaceCmd) -> Result<(Value, String, bool)> {
    require_features(cmd, &client.negotiate().await?.features)?;
    let result = match cmd {
        WorkspaceCmd::Sync(args) => return sync(client, repo, args).await,
        WorkspaceCmd::List => list(client, repo).await,
        WorkspaceCmd::Show { name } => show(client, repo, name).await,
        WorkspaceCmd::Create { name, stream, from } => create(client, repo, name, stream, from.as_deref()).await,
        WorkspaceCmd::Delete { name } => delete(client, repo, name).await,
    };
    result.map(|(value, message)| (value, message, false))
}

async fn list(client: &Client, repo: &str) -> Result<(Value, String)> {
    let result = client
        .workspaces()
        .list(client.req(v1::ListWorkspacesRequest { repo: repo.into() }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let message = if result.workspaces.is_empty() {
        "No server-side workspaces.".into()
    } else {
        result.workspaces.iter().map(describe_workspace).collect::<Vec<_>>().join("\n\n")
    };
    Ok((serde_json::to_value(result)?, message))
}

async fn show(client: &Client, repo: &str, name: &str) -> Result<(Value, String)> {
    let result = client
        .workspaces()
        .get(client.req(v1::GetWorkspaceRequest { repo: repo.into(), name: name.into() }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let message = describe_workspace(&result);
    Ok((serde_json::to_value(result)?, message))
}

async fn create(client: &Client, repo: &str, name: &str, stream: &str, from: Option<&str>) -> Result<(Value, String)> {
    let result = client
        .workspaces()
        .create(client.req(v1::CreateWorkspaceRequest {
            repo: repo.into(),
            name: name.into(),
            stream: stream.into(),
            from_rev: from.unwrap_or_default().into(),
            ..Default::default()
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let message = describe_workspace(&result);
    Ok((serde_json::to_value(result)?, message))
}

async fn delete(client: &Client, repo: &str, name: &str) -> Result<(Value, String)> {
    client
        .workspaces()
        .delete(client.req(v1::DeleteWorkspaceRequest { repo: repo.into(), name: name.into() }))
        .await
        .map_err(ClientError::from)?;
    Ok((json!({}), format!("Deleted server-side workspace {name}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_resolutions_requires_both_pinning_and_structural_support() {
        let command = WorkspaceCmd::Sync(SyncArgs {
            name: "w".into(),
            resolutions: Some("r.json".into()),
            expected_stream_head: Some("head".into()),
        });
        let virtual_only = vec![features::WORKSPACES_VIRTUAL.into()];
        assert!(require_features(&WorkspaceCmd::List, &virtual_only).is_ok());
        assert!(require_features(&command, &virtual_only).unwrap_err().to_string().contains("sync_precondition"));
        let mut supported = virtual_only;
        supported.push(features::WORKSPACES_SYNC_PRECONDITION.into());
        assert!(require_features(&command, &supported).unwrap_err().to_string().contains("structural_resolution"));
        supported.push(features::WORKSPACES_STRUCTURAL_RESOLUTION.into());
        assert!(require_features(&command, &supported).is_ok());
    }

    #[test]
    fn resolution_json_keeps_bytes_and_metadata_and_rejects_ambiguous_writes() {
        let binary =
            parse_resolution(json!({"path":"a","op":"put","content":"AP8=","executable":true,"file_type":"regular"}))
                .unwrap();
        assert_eq!(binary.content, [0, 255]);
        assert!(binary.executable);
        let blob =
            parse_resolution(json!({"path":"link","op":"put","blob_id":"existing","file_type":"symlink"})).unwrap();
        assert_eq!(blob.blob_id, "existing");
        assert_eq!(blob.file_type, "symlink");
        assert_eq!(parse_resolution(json!({"path":"a","op":"delete"})).unwrap().op, "delete");
        for invalid in [
            json!({"path":"a","op":"put","content_base64":"eA=="}),
            json!({"path":"a","op":"put","content":"eA==","blob_id":"existing"}),
            json!({"path":"a","op":"put"}),
            json!({"path":"a","op":"delete","content":""}),
            json!({"path":"a","op":"put","content":"!"}),
            json!({"path":"../a","op":"delete"}),
        ] {
            assert!(parse_resolution(invalid.clone()).is_err(), "{invalid}");
        }
    }
    #[test]
    fn resolution_files_are_nonempty_and_bounded_before_network_io() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choices.json");
        std::fs::write(&path, "[]").unwrap();
        assert!(resolutions(Some(&path)).is_err());
        let mut valid = br#"[{"path":"empty","op":"put","content":""}]"#.to_vec();
        valid.resize(8 * 1024 * 1024, b' ');
        std::fs::write(&path, &valid).unwrap();
        assert_eq!(resolutions(Some(&path)).unwrap().len(), 1);
        valid.push(b' ');
        std::fs::write(&path, &valid).unwrap();
        assert!(resolutions(Some(&path)).unwrap_err().to_string().contains("8 MiB"));
        let oversized: Vec<_> =
            (0..=ag_protocol::limits::MAX_BATCH).map(|i| json!({"path":format!("file-{i}"),"op":"delete"})).collect();
        std::fs::write(&path, serde_json::to_vec(&oversized).unwrap()).unwrap();
        assert!(resolutions(Some(&path)).is_err());
    }
    #[test]
    fn duplicate_resolution_paths_are_rejected_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choices.json");
        std::fs::write(&path, r#"[{"path":"a","op":"put","content":""},{"path":"a","op":"delete"}]"#).unwrap();
        assert!(resolutions(Some(&path)).unwrap_err().to_string().contains("more than once"));
    }
}
