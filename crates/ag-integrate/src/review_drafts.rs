//! Unsubmitted review notes. These neither publish a review nor retain history objects.
use crate::draft_version::{next_version, require_version};
use crate::{Error, Repository, Result};
use ag_storage::meta::{ReviewDraftRow, Tx};
use ag_types::{ChangesetId, Identity};

/// Maximum UTF-8 bytes in one private review draft.
pub const MAX_REVIEW_DRAFT_BYTES: usize = 64 * 1024;

/// A complete draft together with the version its caller inspected.
#[derive(Clone, Debug)]
pub struct ReviewDraftInput<'a> {
    /// Revision inspected by the reviewer; may be an earlier proposal head.
    pub head: &'a ChangesetId,
    /// Intended decision: approve, request_changes, or comment.
    pub decision: &'a str,
    /// Private, unsubmitted notes.
    pub body: &'a str,
    /// Zero means the caller observed no persisted draft.
    pub expected_version: u64,
}

impl Repository {
    /// Reads the exact actor's private notes, including a cleared draft's version.
    pub fn review_draft(&self, proposal: &str, owner: &Identity) -> Result<Option<ReviewDraftRow>> {
        let proposal = self.find_proposal(proposal)?;
        Ok(self.meta.read(|tx| tx.review_draft(&proposal.id, owner.as_str()))?)
    }

    /// Saves private notes atomically; an identical retry keeps the existing version.
    pub fn save_review_draft(
        &self,
        proposal: &str,
        owner: &Identity,
        input: ReviewDraftInput<'_>,
    ) -> Result<ReviewDraftRow> {
        validate_content(&input)?;
        let proposal = self.find_proposal(proposal)?.id;
        self.meta.write_with_error(|tx| save_in_tx(tx, &proposal, owner, &input))
    }

    /// Clears only the inspected private draft, retaining its version to reject late device writes.
    pub fn clear_review_draft(
        &self,
        proposal: &str,
        owner: &Identity,
        expected_version: u64,
    ) -> Result<Option<ReviewDraftRow>> {
        let proposal = self.find_proposal(proposal)?.id;
        self.meta.write_with_error(|tx| {
            let Some(current) = tx.review_draft(&proposal, owner.as_str())? else {
                return Ok(None);
            };
            if current.head.is_none() {
                return Ok(Some(current));
            }
            require_version(current.version, expected_version)?;
            Ok(Some(tx.replace_review_draft(
                &proposal,
                owner.as_str(),
                None,
                "",
                "",
                next_version(current.version)?,
            )?))
        })
    }
}

fn validate_content(input: &ReviewDraftInput<'_>) -> Result<()> {
    if !["approve", "request_changes", "comment"].contains(&input.decision) {
        return Err(Error::Invalid("unknown draft review decision".into()));
    }
    if input.body.len() > MAX_REVIEW_DRAFT_BYTES {
        return Err(Error::Invalid("review draft exceeds 64 KiB".into()));
    }
    Ok(())
}

fn same_content(row: &ReviewDraftRow, input: &ReviewDraftInput<'_>) -> bool {
    row.head.as_ref() == Some(input.head) && row.decision == input.decision && row.body == input.body
}

fn save_in_tx(tx: &Tx<'_>, proposal: &str, owner: &Identity, input: &ReviewDraftInput<'_>) -> Result<ReviewDraftRow> {
    let row = tx.proposal(proposal)?.ok_or_else(|| Error::NotFound(format!("proposal {proposal}")))?;
    if row.state != "open" {
        return Err(Error::Invalid("only an open proposal accepts a review draft".into()));
    }
    let current = tx.review_draft(proposal, owner.as_str())?;
    if let Some(row) = current.as_ref().filter(|row| same_content(row, input)) {
        return Ok(row.clone());
    }
    let version = current.as_ref().map_or(0, |row| row.version);
    require_version(version, input.expected_version)?;
    Ok(tx.replace_review_draft(
        proposal,
        owner.as_str(),
        Some(input.head),
        input.decision,
        input.body,
        next_version(version)?,
    )?)
}
