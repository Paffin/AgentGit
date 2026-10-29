//! Private discussion notes, isolated by the caller's full identity.
use super::db::params;
use super::{MetaError, Tx, parse, ts};
use ag_types::{ChangesetId, Timestamp};
use serde::Serialize;

/// A persisted private draft; a null head is a versioned deletion tombstone.
#[derive(Clone, Debug, Serialize)]
pub struct CommentDraftRow {
    /// Monotonic per-owner, per-proposal revision.
    pub version: u64,
    /// The revision the commenter inspected, or none after clearing.
    pub head: Option<ChangesetId>,
    /// Unsubmitted text; empty on a tombstone.
    pub body: String,
    /// Last change time.
    pub updated_at: Timestamp,
}

impl Tx<'_> {
    /// Returns only the draft matching this exact owner and proposal.
    pub fn comment_draft(&self, proposal: &str, owner: &str) -> Result<Option<CommentDraftRow>, MetaError> {
        let rows = self.conn.query(
            "SELECT version, head, body, updated_at FROM comment_drafts WHERE proposal = ?1 AND owner = ?2",
            &params![proposal, owner],
        )?;
        rows.first()
            .map(|row| {
                Ok(CommentDraftRow {
                    version: row.i64(0)? as u64,
                    head: row.opt_string(1)?.map(|value| parse(&value, "draft head")).transpose()?,
                    body: row.string(2)?,
                    updated_at: ts(row.i64(3)?),
                })
            })
            .transpose()
    }

    /// Writes the state whose version was already compared in this transaction.
    pub fn replace_comment_draft(
        &self,
        proposal: &str,
        owner: &str,
        head: Option<&ChangesetId>,
        body: &str,
        version: u64,
    ) -> Result<CommentDraftRow, MetaError> {
        let version = i64::try_from(version).map_err(|_| MetaError::Invalid("draft version exhausted".into()))?;
        self.conn.execute("INSERT INTO comment_drafts (proposal, owner, version, head, body, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (proposal, owner) DO UPDATE SET version = excluded.version, head = excluded.head, body = excluded.body, updated_at = excluded.updated_at", &params![proposal, owner, version, head.map(|id| id.to_hex()), body, self.now().as_millis()])?;
        self.comment_draft(proposal, owner)?.ok_or_else(|| MetaError::Corrupt("comment draft vanished".into()))
    }
}
