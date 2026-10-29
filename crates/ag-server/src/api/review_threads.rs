//! Revision-bound conversations. Discussion never manufactures an approval.
use super::{decode_keyset, decode_offset, encode_keyset, encode_offset, keyed, page_size, parse_id, require};
use crate::{
    app::{App, Call},
    authz::Cap,
    error::{ApiError, ApiResult},
};
use ag_integrate::Repository;
use ag_protocol::{ErrorCode, v1};
use ag_storage::meta::{MetaError, ReviewThreadAnchorRow, ReviewThreadRow, ThreadCommentRow, Tx};
use ag_types::{ChangesetId, Identity, RepoPath};

fn wire(r: ReviewThreadRow) -> v1::ReviewThread {
    v1::ReviewThread {
        id: r.id,
        proposal: r.proposal,
        anchor: Some(v1::ReviewLineAnchor {
            base: r.anchor.base.map(|id| id.to_string()).unwrap_or_default(),
            head: r.anchor.head.to_string(),
            path: r.anchor.path,
            side: r.anchor.side,
            line: r.anchor.line,
            text: r.anchor.source_text,
        }),
        body: r.body,
        author: r.author,
        resolved: r.resolved,
        version: r.version,
        created_at_ms: r.created_at_ms,
        updated_at_ms: r.updated_at_ms,
    }
}
fn comment(r: ThreadCommentRow) -> v1::ThreadComment {
    v1::ThreadComment {
        id: r.id,
        thread_id: r.thread_id,
        body: r.body,
        author: r.author,
        created_at_ms: r.created_at_ms,
    }
}
fn body(text: &str) -> ApiResult<()> {
    if text.trim().is_empty() || text.len() > 16384 || text.contains('\0') {
        return Err(ApiError::invalid("discussion text must be non-empty and at most 16 KiB"));
    }
    Ok(())
}
fn found(tx: &Tx<'_>, proposal: &str, id: &str) -> Result<ReviewThreadRow, MetaError> {
    tx.review_thread(proposal, id)?.ok_or_else(|| MetaError::NotFound("review thread".into()))
}
fn event(tx: &Tx<'_>, row: &ReviewThreadRow, actor: &Identity, action: &str) -> Result<(), MetaError> {
    let data = serde_json::json!({"proposal":row.proposal,"thread_id":row.id,"head":row.anchor.head,"version":row.version,"resolved":row.resolved});
    tx.audit_append(actor.as_str(), action, &row.id, &data)?;
    tx.outbox_push("proposal", &data)?;
    Ok(())
}
fn anchor_revision(anchor: &v1::ReviewLineAnchor) -> ApiResult<ChangesetId> {
    if !["old", "new"].contains(&anchor.side.as_str()) || anchor.line == 0 {
        return Err(ApiError::invalid("a line anchor requires old/new side and a positive line number"));
    }
    let revision = if anchor.side == "old" { require(&anchor.base, "old-side base")? } else { &anchor.head };
    parse_id(revision, "anchor revision")
}
fn require_published(repo: &Repository, revision: &ChangesetId) -> ApiResult<()> {
    if repo.meta().read(|tx| tx.changeset_info(revision))?.is_none() {
        return Err(ApiError::invalid("anchor revision must be published in this repository"));
    }
    Ok(())
}
fn source_blob(repo: &Repository, anchor: &v1::ReviewLineAnchor) -> ApiResult<Vec<u8>> {
    let revision = anchor_revision(anchor)?;
    require_published(repo, &revision)?;
    let path = RepoPath::new(&anchor.path).map_err(|e| ApiError::invalid(e.to_string()))?;
    let tree = repo.tree_of(&revision)?;
    let entry = tree.get(&path).ok_or_else(|| ApiError::invalid("anchor file does not exist at that revision"))?;
    if repo.blob_len(&entry.content)?.unwrap_or(u64::MAX) > 8 * 1024 * 1024 {
        return Err(ApiError::invalid("line discussions support source files up to 8 MiB"));
    }
    Ok(repo.blob(&entry.content)?)
}
fn source_line(repo: &Repository, anchor: &v1::ReviewLineAnchor) -> ApiResult<String> {
    let bytes = source_blob(repo, anchor)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| ApiError::invalid("line discussions require a UTF-8 text file"))?;
    if text.contains('\0') {
        return Err(ApiError::invalid("line discussions do not support binary files"));
    }
    let line = text
        .lines()
        .nth(anchor.line as usize - 1)
        .ok_or_else(|| ApiError::invalid("anchor line does not exist at that revision"))?;
    if line.len() > 16384 {
        return Err(ApiError::invalid("anchor line exceeds 16 KiB"));
    }
    Ok(line.to_string())
}
struct PreparedThread {
    req: v1::CreateReviewThreadRequest,
    anchor: v1::ReviewLineAnchor,
    expected: ChangesetId,
    base: Option<ChangesetId>,
}
fn optional_base(value: &str) -> ApiResult<Option<ChangesetId>> {
    if value.is_empty() { Ok(None) } else { Ok(Some(parse_id(value, "anchor base")?)) }
}
fn prepare_thread(mut req: v1::CreateReviewThreadRequest) -> ApiResult<PreparedThread> {
    body(&req.body)?;
    let expected = parse_id(require(&req.expected_head, "expected_head")?, "inspected head")?;
    let mut anchor = req.anchor.take().ok_or_else(|| ApiError::invalid("anchor is required"))?;
    if parse_id::<ChangesetId>(&anchor.head, "anchor head")? != expected {
        return Err(ApiError::invalid("anchor must describe the inspected proposal head"));
    }
    let base = optional_base(&anchor.base)?;
    anchor.text.clear();
    Ok(PreparedThread { req, anchor, expected, base })
}
fn append_thread(
    tx: &Tx<'_>,
    proposal: &str,
    actor: &Identity,
    p: &PreparedThread,
    text: &str,
    claim: &super::Claim,
) -> Result<Option<v1::ReviewThread>, MetaError> {
    let current = tx.proposal(proposal)?.ok_or_else(|| MetaError::NotFound("proposal".into()))?;
    let base_ready = match p.base {
        Some(base) => tx.is_ready(&base)?,
        None => true,
    };
    if !tx.is_ready(&p.expected)? || !base_ready {
        return Err(MetaError::Invalid("review source revision is no longer published".into()));
    }
    if current.head != p.expected {
        return Ok(None);
    }
    let row = ReviewThreadRow {
        id: ag_crypto::random_id("thread").map_err(|e| MetaError::Invalid(e.to_string()))?,
        proposal: proposal.into(),
        anchor: ReviewThreadAnchorRow {
            base: p.base,
            head: p.expected,
            path: p.anchor.path.clone(),
            side: p.anchor.side.clone(),
            line: p.anchor.line,
            source_text: text.into(),
        },
        body: p.req.body.clone(),
        author: actor.to_string(),
        resolved: false,
        version: 1,
        created_at_ms: tx.now().as_millis(),
        updated_at_ms: tx.now().as_millis(),
    };
    tx.insert_review_thread(&row)?;
    event(tx, &row, actor, "proposal.thread_create")?;
    let response = wire(row);
    claim.record(tx, &response)?;
    Ok(Some(response))
}
fn append_reply(
    tx: &Tx<'_>,
    proposal: &str,
    req: &v1::AddThreadCommentRequest,
    actor: &Identity,
    claim: &super::Claim,
) -> Result<v1::ThreadComment, MetaError> {
    found(tx, proposal, &req.thread_id)?;
    let response = comment(tx.add_thread_comment(&req.thread_id, &req.body, actor.as_str())?);
    event(tx, &found(tx, proposal, &req.thread_id)?, actor, "proposal.thread_reply")?;
    claim.record(tx, &response)?;
    Ok(response)
}
fn resolve_thread(
    tx: &Tx<'_>,
    proposal: &str,
    req: &v1::ResolveReviewThreadRequest,
    actor: &Identity,
    claim: &super::Claim,
) -> Result<Option<v1::ReviewThread>, MetaError> {
    if !tx.resolve_review_thread(&req.thread_id, req.expected_version, req.resolved)? {
        return Ok(None);
    }
    let row = found(tx, proposal, &req.thread_id)?;
    event(tx, &row, actor, "proposal.thread_resolve")?;
    let response = wire(row);
    claim.record(tx, &response)?;
    Ok(Some(response))
}
fn may_resolve(manage: bool, thread_author: &str, proposal_author: &str, actor: &Identity) -> ApiResult<()> {
    if manage || super::owns(thread_author, actor) || super::owns(proposal_author, actor) {
        return Ok(());
    }
    Err(ApiError::denied("only the thread author, proposal author, reviewer or integrator may resolve it"))
}
async fn reviewer_or_integrator(app: &App, call: &Call, repo: &str, proposal: &str) -> bool {
    app.with_proposal(call, repo, Cap::ProposalReview, proposal.to_string(), |_, _| Ok(())).await.is_ok()
        || app.with_proposal(call, repo, Cap::IntegrateRun, proposal.to_string(), |_, _| Ok(())).await.is_ok()
}
fn cursor_scope(repo: &str, id: &str, kind: &str) -> String {
    use sha2::Digest;
    format!("{kind}-{}", hex::encode(sha2::Sha256::digest(format!("{repo}\0{id}"))))
}
impl App {
    /// Lists anchored threads newest first, retaining resolved and outdated discussions.
    pub async fn list_review_threads(
        &self,
        call: &Call,
        req: v1::ListReviewThreadsRequest,
    ) -> ApiResult<v1::ListReviewThreadsResponse> {
        let size = page_size(req.page_size).min(100);
        let scope = cursor_scope(&req.repo, &req.id, "threads");
        let after = decode_keyset(&scope, &req.page_token)?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProposalRead, None, move |repo, _| {
            let proposal = repo.find_proposal(&req.id)?;
            let mut rows = repo.meta().read(|tx| {
                tx.review_threads(&proposal.id, after.as_ref().map(|(t, id)| (*t, id.as_str())), size + 1)
            })?;
            let more = rows.len() > size;
            rows.truncate(size);
            let next = if more {
                rows.last().map(|r| encode_keyset(&scope, r.created_at_ms, &r.id)).unwrap_or_default()
            } else {
                String::new()
            };
            Ok(v1::ListReviewThreadsResponse { threads: rows.into_iter().map(wire).collect(), next_page_token: next })
        })
        .await
    }
    /// Creates an immutable anchor and first message at an explicitly inspected head.
    pub async fn create_review_thread(
        &self,
        call: &Call,
        req: v1::CreateReviewThreadRequest,
    ) -> ApiResult<v1::ReviewThread> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let prepared = prepare_thread(req)?;
        let idem = self.idem();
        self.with_proposal_mut(call, &repo_name, Cap::ProposalComment, prepared.req.id.clone(), move |repo, actor| {
            let proposal = repo.find_proposal(&prepared.req.id)?;
            if let Some(base) = prepared.base {
                require_published(repo, &base)?;
            }
            let text = source_line(repo, &prepared.anchor)?;
            let fingerprint =
                serde_json::to_string(&(actor.as_str(), &proposal.id, &prepared.anchor, &prepared.req.body))
                    .map_err(ApiError::internal)?;
            keyed(repo, idem, "proposal.thread_create", &prepared.req.idempotency_key, &fingerprint, |claim| {
                let result = repo.meta().write(|tx| append_thread(tx, &proposal.id, actor, &prepared, &text, claim))?;
                result.ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::FailedPrecondition,
                        "proposal head changed; inspect it before opening a thread",
                    )
                })
            })
        })
        .await
    }
    /// Lists immutable replies within a verified proposal/thread pair.
    pub async fn list_thread_comments(
        &self,
        call: &Call,
        req: v1::ListThreadCommentsRequest,
    ) -> ApiResult<v1::ListThreadCommentsResponse> {
        let size = page_size(req.page_size).min(100);
        let scope = cursor_scope(&req.repo, &format!("{}:{}", req.id, req.thread_id), "thread-replies");
        let after = decode_offset(&scope, &req.page_token)? as u64;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProposalRead, None, move |repo, _| {
            let proposal = repo.find_proposal(&req.id)?;
            let mut rows = repo.meta().read(|tx| {
                found(tx, &proposal.id, &req.thread_id)?;
                tx.thread_comments(&req.thread_id, after, size + 1)
            })?;
            let more = rows.len() > size;
            rows.truncate(size);
            let next = if more {
                rows.last().map(|r| encode_offset(&scope, r.id as usize)).unwrap_or_default()
            } else {
                String::new()
            };
            Ok(v1::ListThreadCommentsResponse {
                comments: rows.into_iter().map(comment).collect(),
                next_page_token: next,
            })
        })
        .await
    }
    /// Adds a reply and advances the thread revision, so unseen replies prevent resolution.
    pub async fn reply_review_thread(
        &self,
        call: &Call,
        req: v1::AddThreadCommentRequest,
    ) -> ApiResult<v1::ThreadComment> {
        body(&req.body)?;
        let repo_name = require(&req.repo, "repo")?.to_string();
        let idem = self.idem();
        self.with_proposal_mut(call, &repo_name, Cap::ProposalComment, req.id.clone(), move |repo, actor| {
            let proposal = repo.find_proposal(&req.id)?;
            let fingerprint = serde_json::to_string(&(actor.as_str(), &proposal.id, &req.thread_id, &req.body))
                .map_err(ApiError::internal)?;
            keyed(repo, idem, "proposal.thread_reply", &req.idempotency_key, &fingerprint, |claim| {
                Ok(repo.meta().write(|tx| append_reply(tx, &proposal.id, &req, actor, claim))?)
            })
        })
        .await
    }
    /// Resolves or reopens a discussion using an exact version.
    pub async fn resolve_review_thread(
        &self,
        call: &Call,
        req: v1::ResolveReviewThreadRequest,
    ) -> ApiResult<v1::ReviewThread> {
        if req.expected_version == 0 || req.expected_version >= i64::MAX as u64 {
            return Err(ApiError::invalid("expected_version is required"));
        }
        let manage = reviewer_or_integrator(self, call, &req.repo, &req.id).await;
        let repo_name = require(&req.repo, "repo")?.to_string();
        let idem = self.idem();
        self.with_proposal_mut(call, &repo_name, Cap::ProposalComment, req.id.clone(), move |repo, actor| {
            let proposal = repo.find_proposal(&req.id)?;
            let old = repo.meta().read(|tx| found(tx, &proposal.id, &req.thread_id))?;
            may_resolve(manage, &old.author, &proposal.author, actor)?;
            let fingerprint = serde_json::to_string(&(
                actor.as_str(),
                &proposal.id,
                &req.thread_id,
                req.expected_version,
                req.resolved,
            ))
            .map_err(ApiError::internal)?;
            keyed(repo, idem, "proposal.thread_resolve", &req.idempotency_key, &fingerprint, |claim| {
                let result = repo.meta().write(|tx| resolve_thread(tx, &proposal.id, &req, actor, claim))?;
                result.ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::FailedPrecondition,
                        "thread changed; read the latest replies before changing resolution",
                    )
                })
            })
        })
        .await
    }
}
