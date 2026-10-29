//! Command implementations, grouped by area.

pub mod admin;
pub mod agents;
pub mod auth;
pub mod comment_drafts;
pub mod config;
pub mod conflicts;
pub mod discussions;
pub mod doctor;
pub mod event_feed;
pub mod explain;
pub mod file_drafts;
pub mod flow;
pub mod gitbridge;
pub mod hosting_export;
pub mod issue_import;
pub mod login;
pub mod overview;
pub mod policy;
pub mod remote;
pub mod repo;
pub mod review_drafts;
pub mod reviewers;
pub mod shell;
pub mod topics;
pub mod virtual_workspaces;

use crate::ctx::Ctx;

/// A stream chosen positionally (`ag policy show main`) or with `--stream main`.
///
/// `propose`, `policy set`, `workspace create` and the remote commands always took `--stream`; `policy show`,
/// `policy history`, `queue list|run` and `predict` took it as a positional argument. Both spellings are now
/// accepted by all of them, so a script can use one form everywhere. Giving both is a usage error (exit code 2).
#[derive(clap::Args, Debug, Default)]
pub struct StreamSel {
    /// Stream (default: the default stream); `--stream STREAM` means the same.
    #[arg(value_name = "STREAM", conflicts_with = "stream_flag")]
    stream: Option<String>,
    /// The stream, as an option (the same as the positional STREAM).
    #[arg(long = "stream", id = "stream_flag", value_name = "STREAM")]
    stream_flag: Option<String>,
}

impl StreamSel {
    /// The chosen stream, or the repository's default stream.
    pub fn name(&self, ctx: &Ctx) -> String {
        self.stream.clone().or_else(|| self.stream_flag.clone()).unwrap_or_else(|| ctx.default_stream())
    }
}

/// Read a private draft without allocating an unbounded file into memory.
fn private_draft_body(
    message: &str,
    path: Option<&std::path::Path>,
    limit: usize,
    too_large: &str,
) -> anyhow::Result<String> {
    use anyhow::Context;
    use std::io::Read;
    let bytes = match path {
        Some(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .with_context(|| format!("reading {}", path.display()))?
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("reading {}", path.display()))?;
            bytes
        }
        None => {
            anyhow::ensure!(message.len() <= limit, "{too_large}");
            message.as_bytes().to_vec()
        }
    };
    anyhow::ensure!(bytes.len() <= limit, "{too_large}");
    String::from_utf8(bytes).context("private draft must be UTF-8")
}

async fn require_private_draft_support(client: &ag_client::Client, capability: &str, kind: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        client.negotiate().await?.features.iter().any(|feature| feature == capability),
        "this server does not support private {kind} drafts; update it before using this command"
    );
    Ok(())
}
