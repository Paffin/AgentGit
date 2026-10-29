//! Proposals: open, list, update, review, close.

use ag_protocol::v1;
use ag_types::{ChangesetId, StreamName};

use ag_storage::meta::ProposalFilter;

use super::{convert, decode_keyset, encode_keyset, page_size, parse_id, require};
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
            let task_id = some(&req.task_id);
            let filter = ProposalFilter {
                state: state.as_deref(),
                stream: stream.as_deref(),
                author: author.as_deref(),
                task_id: task_id.as_deref(),
            };
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
        let expected = if req.expected_head.is_empty() {
            None
        } else {
            Some(parse_id::<ChangesetId>(&req.expected_head, "expected_head")?)
        };
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalUpdate, None, move |repo, actor| {
            let p = repo.find_proposal(&req.id)?;
            if !super::owns(&p.author, actor) {
                return Err(ApiError::denied("only the author may update a proposal"));
            }
            let head = repo.resolve_rev(&new_head_rev)?;
            Ok(convert::proposal(&repo.update_proposal_at_head(&p.id, &head, actor, expected.as_ref())?))
        })
        .await
    }

    /// Records a review (approve, request_changes or comment) of the current head.
    pub async fn review_proposal(&self, call: &Call, req: v1::ReviewProposalRequest) -> ApiResult<v1::Review> {
        let decision = require(&req.decision, "decision")?.to_string();
        let expected_head = if req.expected_head.is_empty() {
            None
        } else {
            Some(parse_id(&req.expected_head, "reviewed changeset id")?)
        };
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalReview, None, move |repo, actor| {
            Ok(convert::review(&repo.review_at_head(&req.id, actor, &decision, &req.body, expected_head.as_ref())?))
        })
        .await
    }

    /// Adds discussion without granting approval privileges. Works on closed proposals too.
    pub async fn comment_proposal(&self, call: &Call, req: v1::CommentProposalRequest) -> ApiResult<v1::Review> {
        validate_comment_body(&req.body)?;
        let expected: ChangesetId = parse_id(require(&req.expected_head, "expected_head")?, "inspected proposal head")?;
        let idem = self.idem();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProposalComment, None, move |repo, actor| {
            let id = repo.find_proposal(&req.id)?.id;
            let fingerprint = serde_json::to_string(&(&id, actor.as_str(), &req.body, expected.to_hex()))
                .map_err(ApiError::internal)?;
            super::keyed(repo, idem, "proposal.comment", &req.idempotency_key, &fingerprint, |claim| {
                let result = repo.meta().write(|tx| append_comment(tx, &id, actor, &req.body, &expected, claim))?;
                result.ok_or_else(|| {
                    ApiError::new(
                        ag_protocol::ErrorCode::FailedPrecondition,
                        "proposal head changed; inspect it before posting the comment",
                    )
                })
            })
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

fn validate_comment_body(body: &str) -> ApiResult<()> {
    if body.trim().is_empty() || body.len() > 16384 || body.contains('\0') {
        return Err(ApiError::invalid("comment must be non-empty and at most 16 KiB"));
    }
    Ok(())
}

fn append_comment(
    tx: &ag_storage::meta::Tx<'_>,
    id: &str,
    actor: &ag_types::Identity,
    body: &str,
    expected: &ChangesetId,
    claim: &super::Claim,
) -> Result<Option<v1::Review>, ag_storage::meta::MetaError> {
    use ag_storage::meta::MetaError;
    let proposal = tx.proposal(id)?.ok_or_else(|| MetaError::NotFound("proposal".into()))?;
    if proposal.head != *expected {
        return Ok(None);
    }
    let comment_id = tx.review_add(id, actor.as_str(), "comment", body, expected)?;
    let stored = tx.review(comment_id)?.ok_or_else(|| MetaError::Corrupt("comment vanished".into()))?;
    let response = convert::review(&stored);
    tx.audit_append(
        actor.as_str(),
        "proposal.comment",
        id,
        &serde_json::json!({"head":expected,"comment_id":comment_id}),
    )?;
    claim.record(tx, &response)?;
    Ok(Some(response))
}
