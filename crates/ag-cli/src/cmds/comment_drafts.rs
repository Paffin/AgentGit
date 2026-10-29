//! Private account comment notes; saving never posts a comment.
use std::path::PathBuf;

use ag_client::{Client, ClientError};
use ag_protocol::v1;
use anyhow::Result;
use clap::{Args, Subcommand};

/// Read or explicitly replace the signed-in account's comment draft.
#[derive(Subcommand, Debug)]
pub enum CommentDraftCmd {
    /// Read your private draft and its version, including a cleared version.
    Get {
        /// Proposal id or unique prefix.
        id: String,
    },
    /// Save private notes; does not post a comment or advance the inspected head.
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
    /// Private comment notes.
    #[arg(short, long, default_value = "", conflicts_with = "body_file")]
    message: String,
    /// Read UTF-8 notes from this file, preserving line breaks (at most 16 KiB).
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,
    /// Version returned by get; use zero only after observing no draft.
    #[arg(long)]
    expected_version: u64,
}

impl CommentDraftCmd {
    /// Whether this command explicitly changes stored notes.
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::Get { .. })
    }
}

fn body(args: &SaveArgs) -> Result<String> {
    super::private_draft_body(
        &args.message,
        args.body_file.as_deref(),
        ag_integrate::MAX_COMMENT_DRAFT_BYTES,
        "comment draft exceeds 16 KiB",
    )
}

async fn require_support(client: &Client) -> Result<()> {
    super::require_private_draft_support(client, ag_protocol::features::PROPOSALS_COMMENT_DRAFTS, "comment").await
}

/// Run against the authenticated account, without an owner selector or automatic submission.
pub async fn run(client: &Client, repo: &str, cmd: &CommentDraftCmd) -> Result<v1::CommentDraft> {
    require_support(client).await?;
    let mut proposals = client.proposals();
    let response = match cmd {
        CommentDraftCmd::Get { id } => {
            proposals
                .get_comment_draft(client.req(v1::GetCommentDraftRequest { repo: repo.into(), id: id.clone() }))
                .await
        }
        CommentDraftCmd::Save(args) => {
            proposals
                .save_comment_draft(client.req(v1::SaveCommentDraftRequest {
                    repo: repo.into(),
                    id: args.id.clone(),
                    head: args.head.clone(),
                    body: body(args)?,
                    expected_version: args.expected_version,
                }))
                .await
        }
        CommentDraftCmd::Clear { id, expected_version } => {
            proposals
                .clear_comment_draft(client.req(v1::ClearCommentDraftRequest {
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
pub fn describe(draft: &v1::CommentDraft) -> String {
    if !draft.present {
        return format!("No private comment draft. Version {}.", draft.version);
    }
    format!("Private comment draft, version {}.\nInspected head: {}\n\n{}", draft.version, draft.head, draft.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_report_mutation_and_display_the_version_needed_for_the_next_write() {
        assert!(!CommentDraftCmd::Get { id: "p".into() }.changes_state());
        assert!(CommentDraftCmd::Clear { id: "p".into(), expected_version: 3 }.changes_state());
        let mut draft = v1::CommentDraft { version: 3, ..Default::default() };
        assert_eq!(describe(&draft), "No private comment draft. Version 3.");
        draft.present = true;
        draft.head = "inspected".into();
        draft.body = "Private notes".into();
        assert_eq!(describe(&draft), "Private comment draft, version 3.\nInspected head: inspected\n\nPrivate notes");
    }

    #[test]
    fn notes_keep_utf8_and_enforce_the_byte_limit_for_messages_and_files() {
        let mut args = SaveArgs {
            id: "p".into(),
            head: "head".into(),
            message: "é".repeat(8 * 1024),
            body_file: None,
            expected_version: 0,
        };
        assert_eq!(body(&args).unwrap(), args.message);
        args.message.push('x');
        assert!(body(&args).unwrap_err().to_string().contains("16 KiB"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "first\r\nsecond\n").unwrap();
        args.body_file = Some(path.clone());
        assert_eq!(body(&args).unwrap(), "first\r\nsecond\n");
        std::fs::write(&path, [0xff]).unwrap();
        assert!(body(&args).unwrap_err().to_string().contains("UTF-8"));

        std::fs::write(&path, "x".repeat(16 * 1024 + 1)).unwrap();
        assert!(body(&args).unwrap_err().to_string().contains("16 KiB"));
    }
}
