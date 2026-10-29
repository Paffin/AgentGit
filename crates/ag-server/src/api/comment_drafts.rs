//! Owner-private draft operations intentionally do not notify shared repository events.
use super::{parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::ApiResult;
use ag_integrate::CommentDraftInput;
use ag_protocol::v1;
use ag_storage::meta::CommentDraftRow;
use ag_types::ChangesetId;

fn draft(row: Option<&CommentDraftRow>) -> v1::CommentDraft {
    let Some(row) = row else {
        return v1::CommentDraft::default();
    };
    v1::CommentDraft {
        present: row.head.is_some(),
        version: row.version,
        head: row.head.map(|head| head.to_string()).unwrap_or_default(),
        body: row.body.clone(),
        updated_at_ms: row.updated_at.as_millis(),
    }
}

impl App {
    /// Reads only the acting identity's private draft.
    pub async fn get_comment_draft(&self, call: &Call, req: v1::GetCommentDraftRequest) -> ApiResult<v1::CommentDraft> {
        self.with_proposal(
            call,
            require(&req.repo, "repo")?,
            Cap::ProposalComment,
            req.id.clone(),
            move |repo, actor| Ok(draft(repo.comment_draft(&req.id, actor)?.as_ref())),
        )
        .await
    }
    /// Saves private notes with an inspected-version comparison, never a review decision.
    pub async fn save_comment_draft(
        &self,
        call: &Call,
        req: v1::SaveCommentDraftRequest,
    ) -> ApiResult<v1::CommentDraft> {
        let head: ChangesetId = parse_id(require(&req.head, "head")?, "draft head")?;
        self.with_proposal(
            call,
            require(&req.repo, "repo")?,
            Cap::ProposalComment,
            req.id.clone(),
            move |repo, actor| {
                let row = repo.save_comment_draft(
                    &req.id,
                    actor,
                    CommentDraftInput { head: &head, body: &req.body, expected_version: req.expected_version },
                )?;
                Ok(draft(Some(&row)))
            },
        )
        .await
    }
    /// Clears only the inspected private draft and retains its version tombstone.
    pub async fn clear_comment_draft(
        &self,
        call: &Call,
        req: v1::ClearCommentDraftRequest,
    ) -> ApiResult<v1::CommentDraft> {
        self.with_proposal(
            call,
            require(&req.repo, "repo")?,
            Cap::ProposalComment,
            req.id.clone(),
            move |repo, actor| Ok(draft(repo.clear_comment_draft(&req.id, actor, req.expected_version)?.as_ref())),
        )
        .await
    }
}
