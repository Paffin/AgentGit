//! Unsubmitted discussion notes. These neither publish a review nor retain history objects.
use crate::draft_version::{next_version, require_version};
use crate::{Error, Repository, Result};
use ag_storage::meta::{CommentDraftRow, Tx};
use ag_types::{ChangesetId, Identity};

/// Maximum UTF-8 bytes in one private comment draft.
pub const MAX_COMMENT_DRAFT_BYTES: usize = 16 * 1024;

/// A complete draft together with the version its caller inspected.
#[derive(Clone, Debug)]
pub struct CommentDraftInput<'a> {
    /// Revision inspected by the commenter; may be an earlier proposal head.
    pub head: &'a ChangesetId,
    /// Private, unsubmitted notes.
    pub body: &'a str,
    /// Zero means the caller observed no persisted draft.
    pub expected_version: u64,
}

impl Repository {
    /// Reads the exact actor's private notes, including a cleared draft's version.
    pub fn comment_draft(&self, proposal: &str, owner: &Identity) -> Result<Option<CommentDraftRow>> {
        let proposal = self.find_proposal(proposal)?;
        Ok(self.meta.read(|tx| tx.comment_draft(&proposal.id, owner.as_str()))?)
    }

    /// Saves private notes atomically; an identical retry keeps the existing version.
    pub fn save_comment_draft(
        &self,
        proposal: &str,
        owner: &Identity,
        input: CommentDraftInput<'_>,
    ) -> Result<CommentDraftRow> {
        validate_content(&input)?;
        let proposal = self.find_proposal(proposal)?.id;
        self.meta.write_with_error(|tx| save_in_tx(tx, &proposal, owner, &input))
    }

    /// Clears only the inspected private draft, retaining its version to reject late device writes.
    pub fn clear_comment_draft(
        &self,
        proposal: &str,
        owner: &Identity,
        expected_version: u64,
    ) -> Result<Option<CommentDraftRow>> {
        let proposal = self.find_proposal(proposal)?.id;
        self.meta.write_with_error(|tx| {
            let Some(current) = tx.comment_draft(&proposal, owner.as_str())? else {
                return Ok(None);
            };
            if current.head.is_none() {
                return Ok(Some(current));
            }
            require_version(current.version, expected_version)?;
            Ok(Some(tx.replace_comment_draft(&proposal, owner.as_str(), None, "", next_version(current.version)?)?))
        })
    }
}

fn validate_content(input: &CommentDraftInput<'_>) -> Result<()> {
    if input.body.contains('\0') {
        return Err(Error::Invalid("comment draft contains NUL".into()));
    }
    if input.body.len() > MAX_COMMENT_DRAFT_BYTES {
        return Err(Error::Invalid("comment draft exceeds 16 KiB".into()));
    }
    Ok(())
}

fn same_content(row: &CommentDraftRow, input: &CommentDraftInput<'_>) -> bool {
    row.head.as_ref() == Some(input.head) && row.body == input.body
}

fn save_in_tx(tx: &Tx<'_>, proposal: &str, owner: &Identity, input: &CommentDraftInput<'_>) -> Result<CommentDraftRow> {
    tx.proposal(proposal)?.ok_or_else(|| Error::NotFound(format!("proposal {proposal}")))?;
    let current = tx.comment_draft(proposal, owner.as_str())?;
    if let Some(row) = current.as_ref().filter(|row| same_content(row, input)) {
        return Ok(row.clone());
    }
    let version = current.as_ref().map_or(0, |row| row.version);
    require_version(version, input.expected_version)?;
    Ok(tx.replace_comment_draft(proposal, owner.as_str(), Some(input.head), input.body, next_version(version)?)?)
}
