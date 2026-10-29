//! Private replacement bytes, bound to an inspected file revision.
use std::io::Read;
use std::path::PathBuf;

use ag_client::{Client, ClientError};
use ag_protocol::{features, v1};
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

/// Explicit private file draft operations; none publish a changeset.
#[derive(Subcommand, Debug)]
pub enum FileDraftCmd {
    /// List your saved file drafts in a stream, without downloading their content.
    List {
        /// Stream containing the files.
        #[arg(long)]
        stream: String,
    },
    /// Read metadata; download exact replacement bytes only with --output.
    Get {
        #[command(flatten)]
        target: Target,
        /// Write saved bytes to this local file (replaces an existing file).
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Save private replacement bytes for an existing file at an immutable revision.
    Save {
        #[command(flatten)]
        target: Target,
        /// Exact changeset containing the original file you inspected.
        #[arg(long)]
        base: String,
        /// Binary replacement file, at most 4 MiB.
        #[arg(long, value_name = "PATH")]
        content_file: PathBuf,
        /// Version returned by get; zero only if no draft has existed.
        #[arg(long)]
        expected_version: u64,
    },
    /// Clear only the draft version you inspected.
    Clear {
        #[command(flatten)]
        target: Target,
        /// Version returned by get, including a cleared version.
        #[arg(long)]
        expected_version: u64,
    },
}

/// The stable key of an account's private file draft.
#[derive(Args, Debug)]
pub struct Target {
    /// Stream containing the file.
    #[arg(long)]
    stream: String,
    /// Repository-relative file path.
    #[arg(long)]
    path: String,
}

impl FileDraftCmd {
    /// Whether this command mutates the remote draft.
    pub fn changes_state(&self) -> bool {
        matches!(self, Self::Save { .. } | Self::Clear { .. })
    }
}

fn read_content(path: &PathBuf) -> Result<Vec<u8>> {
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let mut content = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("reading {}", path.display()))?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut content)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(content.len() as u64 <= MAX_BYTES, "file draft exceeds 4 MiB");
    Ok(content)
}

fn metadata(draft: &v1::FileDraft) -> Value {
    json!({
        "present": draft.present, "version": draft.version,
        "stream": draft.stream, "path": draft.path, "base": draft.base,
        "original_blob_id": draft.original_blob_id, "file_type": draft.file_type,
        "executable": draft.executable, "content_bytes": draft.content.len(),
        "updated_at_ms": draft.updated_at_ms,
    })
}

/// Operate only on the authenticated account; content never goes to stdout.
pub async fn run(client: &Client, repo: &str, cmd: &FileDraftCmd) -> Result<Value> {
    ensure!(
        client.negotiate().await?.features.iter().any(|feature| feature == features::FILES_DRAFTS),
        "this server does not support private file drafts; update it before using this command"
    );
    let mut drafts = client.file_drafts();
    let response = match cmd {
        FileDraftCmd::List { stream } => {
            let response = drafts
                .list(client.req(v1::ListFileDraftsRequest { repo: repo.into(), stream: stream.clone() }))
                .await
                .map_err(ClientError::from)?
                .into_inner();
            return Ok(serde_json::to_value(response)?);
        }
        FileDraftCmd::Get { target, .. } => {
            drafts
                .get(client.req(v1::GetFileDraftRequest {
                    repo: repo.into(),
                    stream: target.stream.clone(),
                    path: target.path.clone(),
                }))
                .await
        }
        FileDraftCmd::Save { target, base, content_file, expected_version } => {
            drafts
                .save(client.req(v1::SaveFileDraftRequest {
                    repo: repo.into(),
                    stream: target.stream.clone(),
                    path: target.path.clone(),
                    base: base.clone(),
                    content: read_content(content_file)?,
                    expected_version: *expected_version,
                }))
                .await
        }
        FileDraftCmd::Clear { target, expected_version } => {
            drafts
                .clear(client.req(v1::ClearFileDraftRequest {
                    repo: repo.into(),
                    stream: target.stream.clone(),
                    path: target.path.clone(),
                    expected_version: *expected_version,
                }))
                .await
        }
    };
    let draft = response.map_err(ClientError::from)?.into_inner();
    if let FileDraftCmd::Get { output: Some(path), .. } = cmd {
        ensure!(draft.present, "no private file draft to download; version {}", draft.version);
        std::fs::write(path, &draft.content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(metadata(&draft))
}

/// A concise summary with versions needed for explicit follow-up writes.
pub fn describe(value: &Value) -> String {
    if let Some(drafts) = value["drafts"].as_array() {
        if drafts.is_empty() {
            return "No private file drafts in this stream.".into();
        }
        return drafts
            .iter()
            .map(|draft| {
                format!(
                    "{}  version {}  base {}",
                    draft["path"].as_str().unwrap_or_default(),
                    draft["version"],
                    draft["base"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    format!(
        "Private file draft {}: {} (version {}, {} bytes).",
        value["path"].as_str().unwrap_or_default(),
        if value["present"] == true { "saved" } else { "absent" },
        value["version"],
        value["content_bytes"]
    )
}
