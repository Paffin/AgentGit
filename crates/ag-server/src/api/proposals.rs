//! Proposals: open, list, update, review, close.

use ag_protocol::v1;
use ag_types::{ChangesetId, StreamName};

use super::{convert, paginate, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

impl App {
    /// Opens a proposal for a revision (publishing it first if needed).
    pub async fn create_proposal(&self, call: &Call, req: v1::CreateProposalRequest) -> ApiResult<v1::Proposal> {
        let stream = StreamName::new(&req.stream).map_err(|e| ApiError::invalid(e.to_string()))?;
        require(&req.head, "head")?;
        let max_open = self.repo_row(&req.repo).await?.map(|r| r.quota.max_open_proposals).unwrap_or(0);
        let (head_rev, title, desc) = (req.head.clone(), req.title.clone(), req.description.clone());
        let task = (!req.task_id.is_empty()).then(|| req.task_id.clone());
        let key = req.idempotency_key.clone();
        let stream_s = stream.to_string();
        self.with_repo_mut(
            call,
            require(&req.repo, "repo")?,
            Cap::ProposalCreate,
            Some(&stream_s),
            move |repo, actor| {
                if max_open > 0
                    && repo.meta().read(|tx| tx.proposals_list(Some("open"), 100_000))?.len() as u32 >= max_open
                {
                    return Err(ApiError::new(
                        ag_protocol::ErrorCode::QuotaExceeded,
                        format!("this repository is limited to {max_open} open proposals"),
                    ));
                }
                let head: ChangesetId = repo.resolve_rev(&head_rev)?;
                let key = (!key.is_empty()).then_some(key.as_str());
                let (row, _) =
                    repo.propose_idempotent(stream.as_str(), &head, &title, &desc, actor, task.as_deref(), key)?;
                Ok(convert::proposal(&row))
            },
        )
        .await
    }

    /// One proposal with its reviews.
    pub async fn get_proposal(&self, call: &Call, req: v1::GetProposalRequest) -> ApiResult<v1::ProposalDetail> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProposalRead, None, move |repo, _| {
            let p = repo.find_proposal(&req.id)?;
            let reviews = repo.meta().read(|tx| tx.reviews(&p.id))?;
            Ok(v1::ProposalDetail {
                proposal: Some(convert::proposal(&p)),
                reviews: reviews.iter().map(convert::review).collect(),
            })
        })
        .await
    }

    /// Lists proposals (newest first).
    pub async fn list_proposals(
        &self,
        call: &Call,
        req: v1::ListProposalsRequest,
    ) -> ApiResult<v1::ListProposalsResponse> {
        let (size, token) = (req.page_size, req.page_token.clone());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProposalRead, None, move |repo, _| {
            let state = (!req.state.is_empty()).then_some(req.state.as_str());
            let mut rows = repo.meta().read(|tx| tx.proposals_list(state, 100_000))?;
            rows.retain(|p| {
                (req.stream.is_empty() || p.stream == req.stream) && (req.author.is_empty() || p.author == req.author)
            });
            let (page, next) = paginate("prop", rows, size, &token)?;
            Ok(v1::ListProposalsResponse {
                proposals: page.iter().map(convert::proposal).collect(),
                next_page_token: next,
            })
        })
        .await
    }

    /// Moves a proposal to a newer head.
    pub async fn update_proposal(&self, call: &Call, req: v1::UpdateProposalRequest) -> ApiResult<v1::Proposal> {
        let new_head_rev = require(&req.new_head, "new_head")?.to_string();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalUpdate, None, move |repo, actor| {
            let p = repo.find_proposal(&req.id)?;
            if p.author != actor.to_string()
                && ag_types::Identity::new(p.author.clone()).map(|a| a.principal() != actor.principal()).unwrap_or(true)
            {
                return Err(ApiError::denied("only the author may update a proposal"));
            }
            let head = repo.resolve_rev(&new_head_rev)?;
            Ok(convert::proposal(&repo.update_proposal(&p.id, &head, actor)?))
        })
        .await
    }

    /// Records a review (approve, request_changes or comment) of the current head.
    pub async fn review_proposal(&self, call: &Call, req: v1::ReviewProposalRequest) -> ApiResult<v1::Review> {
        let decision = require(&req.decision, "decision")?.to_string();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalReview, None, move |repo, actor| {
            Ok(convert::review(&repo.review(&req.id, actor, &decision, &req.body)?))
        })
        .await
    }

    /// Closes an open proposal (the author, or anyone who may close proposals).
    pub async fn close_proposal(&self, call: &Call, req: v1::CloseProposalRequest) -> ApiResult<v1::Proposal> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalClose, None, move |repo, actor| {
            let p = repo.find_proposal(&req.id)?;
            repo.close_proposal(&p.id, actor)?;
            Ok(convert::proposal(&repo.find_proposal(&p.id)?))
        })
        .await
    }
}
