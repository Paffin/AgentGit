//! Private account review notes; saving never publishes a review.
use std::path::PathBuf;

use ag_client::{Client, ClientError};
use ag_protocol::v1;
use anyhow::Result;
use clap::{Args, Subcommand};

/// Read or explicitly replace the signed-in account's review draft.
#[derive(Subcommand, Debug)]
pub enum ReviewDraftCmd {
    /// Read your private draft and its version, including a cleared version.
    Get {
        /// Proposal id or unique prefix.
        id: String,
    },
    /// Save private notes; does not submit a review or advance the inspected head.
    Save(SaveArgs),
    /// Clear only the private draft version you inspected.
    Clear {
        /// Proposal id or unique prefix.
        id: String,
        /// Version returned by get; zero means no draft has existed.
        #[arg(long)]
        expected_version: u64,
    },
}

/// An explicit replacement, never an implicit fetch-and-overwrite.
#[derive(Args, Debug)]
pub struct SaveArgs {
    /// Proposal id or unique prefix.
    id: String,
    /// Exact proposal head you inspected.
    #[arg(long)]
    head: String,
    /// Intended review decision; saving does not submit it.
    #[arg(long, value_parser = ["approve", "request_changes", "comment"])]
    decision: String,
    /// Private review notes.
    #[arg(short, long, default_value = "", conflicts_with = "body_file")]
    message: String,
    /// Read UTF-8 notes from this file, preserving line breaks (at most 64 KiB).
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,
    /// Version returned by get; use zero only after observing no draft.
    #[arg(long)]
    expected_version: u64,
}

impl ReviewDraftCmd {
    /// Whether this command explicitly changes stored notes.
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::Get { .. })
    }
}

fn body(args: &SaveArgs) -> Result<String> {
    super::private_draft_body(
        &args.message,
        args.body_file.as_deref(),
        ag_integrate::MAX_REVIEW_DRAFT_BYTES,
        "review draft exceeds 64 KiB",
    )
}

async fn require_support(client: &Client) -> Result<()> {
    super::require_private_draft_support(client, ag_protocol::features::PROPOSALS_REVIEW_DRAFTS, "review").await
}

/// Run against the authenticated account, without an owner selector or automatic submission.
pub async fn run(client: &Client, repo: &str, cmd: &ReviewDraftCmd) -> Result<v1::ReviewDraft> {
    require_support(client).await?;
    let mut proposals = client.proposals();
    let response = match cmd {
        ReviewDraftCmd::Get { id } => {
            proposals.get_draft(client.req(v1::GetReviewDraftRequest { repo: repo.into(), id: id.clone() })).await
        }
        ReviewDraftCmd::Save(args) => {
            proposals
                .save_draft(client.req(v1::SaveReviewDraftRequest {
                    repo: repo.into(),
                    id: args.id.clone(),
                    head: args.head.clone(),
                    decision: args.decision.clone(),
                    body: body(args)?,
                    expected_version: args.expected_version,
                }))
                .await
        }
        ReviewDraftCmd::Clear { id, expected_version } => {
            proposals
                .clear_draft(client.req(v1::ClearReviewDraftRequest {
                    repo: repo.into(),
                    id: id.clone(),
                    expected_version: *expected_version,
                }))
                .await
        }
    };
    Ok(response.map_err(ClientError::from)?.into_inner())
}

/// Show the version needed for the next explicit save or clear.
pub fn describe(draft: &v1::ReviewDraft) -> String {
    if !draft.present {
        return format!("No private review draft. Version {}.", draft.version);
    }
    format!(
        "Private review draft, version {}.\nDecision: {}\nInspected head: {}\n\n{}",
        draft.version, draft.decision, draft.head, draft.body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_utf8_and_enforce_the_byte_limit_for_messages_and_files() {
        let mut args = SaveArgs {
            id: "p".into(),
            head: "head".into(),
            decision: "comment".into(),
            message: "é".repeat(32 * 1024),
            body_file: None,
            expected_version: 0,
        };
        assert_eq!(body(&args).unwrap(), args.message);
        args.message.push('x');
        assert!(body(&args).unwrap_err().to_string().contains("64 KiB"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "first\r\nsecond\n").unwrap();
        args.body_file = Some(path.clone());
        assert_eq!(body(&args).unwrap(), "first\r\nsecond\n");
        std::fs::write(&path, "x".repeat(64 * 1024 + 1)).unwrap();
        assert!(body(&args).unwrap_err().to_string().contains("64 KiB"));
    }
}
