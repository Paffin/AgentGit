//! Proposals: open, list, update, review, close.

use ag_protocol::v1;
use ag_types::{ChangesetId, StreamName};

use ag_storage::meta::ProposalFilter;

use super::{convert, decode_keyset, encode_keyset, page_size, require};
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
                let head: ChangesetId = repo.resolve_rev(&head_rev)?;
                let key = (!key.is_empty()).then_some(key.as_str());
                // The limit is counted inside the transaction that inserts the proposal.
                let cap = (max_open > 0).then_some(u64::from(max_open));
                let (row, _) =
                    repo.propose_capped(stream.as_str(), &head, &title, &desc, actor, task.as_deref(), key, cap)?;
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
        let size = page_size(req.page_size);
        let after = decode_keyset("prop", &req.page_token)?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProposalRead, None, move |repo, _| {
            let some = |s: &str| (!s.is_empty()).then_some(s.to_string());
            let (state, stream, author) = (some(&req.state), some(&req.stream), some(&req.author));
            let filter =
                ProposalFilter { state: state.as_deref(), stream: stream.as_deref(), author: author.as_deref() };
            let after = after.as_ref().map(|(at, id)| (*at, id.as_str()));
            // One more than a page, to learn whether there is a next one.
            let mut rows = repo.meta().read(|tx| tx.proposals_page(&filter, after, size + 1))?;
            let next = if rows.len() > size {
                rows.truncate(size);
                rows.last().map(|p| encode_keyset("prop", p.created_at.as_millis(), &p.id)).unwrap_or_default()
            } else {
                String::new()
            };
            Ok(v1::ListProposalsResponse {
                proposals: rows.iter().map(convert::proposal).collect(),
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
            if !super::owns(&p.author, actor) {
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
            // One run does not close what another run of its agent proposed.
            if p.author.contains('#') && actor.as_str().contains('#') && !super::owns(&p.author, actor) {
                return Err(ApiError::denied(format!("proposal {} was made by {}", p.id, p.author)));
            }
            repo.close_proposal(&p.id, actor)?;
            Ok(convert::proposal(&repo.find_proposal(&p.id)?))
        })
        .await
    }
}
