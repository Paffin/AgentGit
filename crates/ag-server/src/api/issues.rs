//! User issues: repository-scoped, durable, audited, and separate from agent tasks.
use super::{decode_offset, encode_offset, keyed, page_size, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};
use ag_protocol::{ErrorCode, v1};
use ag_storage::meta::{IssueActivityRow, IssueFilter, IssueRow, MetaError, Tx};
use ag_types::Identity;
use sha2::Digest;

fn wire(r: IssueRow) -> v1::Issue {
    v1::Issue {
        number: r.number,
        title: r.title,
        body: r.body,
        author: r.author,
        state: r.state,
        created_at_ms: r.created_at_ms,
        updated_at_ms: r.updated_at_ms,
        version: r.revision,
        task_id: r.task_id.unwrap_or_default(),
    }
}
fn activity(r: IssueActivityRow) -> v1::IssueActivity {
    v1::IssueActivity {
        id: r.id,
        issue_number: r.issue_number,
        kind: r.kind,
        body: r.body,
        author: r.author,
        created_at_ms: r.created_at_ms,
    }
}
fn number(n: u64) -> ApiResult<()> {
    if n == 0 || n > i64::MAX as u64 {
        return Err(ApiError::invalid("invalid issue number"));
    }
    Ok(())
}
fn text(title: &str, body: &str) -> ApiResult<()> {
    if title.trim().is_empty() || title.len() > 240 || title.contains(['\n', '\r', '\0']) {
        return Err(ApiError::invalid("title must be one non-empty line, at most 240 UTF-8 bytes"));
    }
    if body.len() > 65536 || body.contains('\0') {
        return Err(ApiError::invalid("description must be at most 64 KiB and contain no NUL"));
    }
    Ok(())
}
fn task(tx: &Tx<'_>, id: &str) -> Result<(), MetaError> {
    if !id.is_empty() && tx.task(id)?.is_none() {
        return Err(MetaError::Invalid("linked task does not exist in this repository".into()));
    }
    Ok(())
}
fn found(tx: &Tx<'_>, n: u64) -> Result<IssueRow, MetaError> {
    tx.issue(n)?.ok_or_else(|| MetaError::Invalid("issue does not exist".into()))
}
fn record(
    tx: &Tx<'_>,
    row: &IssueRow,
    actor: &Identity,
    kind: &str,
    body: &str,
) -> Result<v1::IssueActivity, MetaError> {
    let event = activity(tx.add_issue_activity(row.number, kind, body, actor.as_str())?);
    let details = serde_json::json!({"number":row.number,"version":row.revision,"kind":kind,"task_id":row.task_id});
    tx.audit_append(actor.as_str(), &format!("issue.{kind}"), &format!("issue:{}", row.number), &details)?;
    tx.outbox_push("issue", &details)?;
    Ok(event)
}
impl App {
    /// Creates an issue and its initial activity atomically.
    pub async fn create_issue(&self, call: &Call, req: v1::CreateIssueRequest) -> ApiResult<v1::Issue> {
        text(&req.title, &req.body)?;
        let idem = self.idem();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::IssueWrite, None, move |repo, actor| {
            let fingerprint = serde_json::to_string(&(&req.title, &req.body, &req.task_id, actor.as_str()))
                .map_err(ApiError::internal)?;
            keyed(repo, idem, "issue.create", &req.idempotency_key, &fingerprint, |claim| {
                Ok(repo.meta().write(|tx| {
                    task(tx, &req.task_id)?;
                    let row = tx.insert_issue(
                        req.title.trim(),
                        &req.body,
                        actor.as_str(),
                        (!req.task_id.is_empty()).then_some(req.task_id.as_str()),
                    )?;
                    record(tx, &row, actor, "created", "")?;
                    let response = wire(row);
                    claim.record(tx, &response)?;
                    Ok(response)
                })?)
            })
        })
        .await
    }
    /// Reads an issue after repository authorization.
    pub async fn get_issue(&self, call: &Call, req: v1::GetIssueRequest) -> ApiResult<v1::Issue> {
        number(req.number)?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::IssueRead, None, move |repo, _| {
            repo.meta().read(|tx| tx.issue(req.number))?.map(wire).ok_or_else(|| ApiError::not_found("issue not found"))
        })
        .await
    }
    /// Lists issues with bounded keyset pagination and literal text filtering.
    pub async fn list_issues(&self, call: &Call, req: v1::ListIssuesRequest) -> ApiResult<v1::ListIssuesResponse> {
        if !["", "open", "closed"].contains(&req.state.as_str()) || req.query.len() > 256 {
            return Err(ApiError::invalid("invalid issue filter"));
        }
        let size = page_size(req.page_size).min(100);
        let cursor_scope = format!(
            "issues-{}",
            hex::encode(sha2::Sha256::digest(
                serde_json::to_vec(&(&req.repo, &req.state, &req.query)).map_err(ApiError::internal)?
            ))
        );
        let before = decode_offset(&cursor_scope, &req.page_token)? as u64;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::IssueRead, None, move |repo, _| {
            let mut rows = repo
                .meta()
                .read(|tx| tx.issues(IssueFilter { state: &req.state, query: &req.query, before, limit: size + 1 }))?;
            let more = rows.len() > size;
            rows.truncate(size);
            let next =
                if more { encode_offset(&cursor_scope, rows.last().unwrap().number as usize) } else { String::new() };
            Ok(v1::ListIssuesResponse { issues: rows.into_iter().map(wire).collect(), next_page_token: next })
        })
        .await
    }
    /// Updates an author-owned issue only at the expected revision.
    pub async fn update_issue(&self, call: &Call, req: v1::UpdateIssueRequest) -> ApiResult<v1::Issue> {
        number(req.number)?;
        text(&req.title, &req.body)?;
        if !["open", "closed"].contains(&req.state.as_str())
            || (req.expected_version == 0 || req.expected_version >= i64::MAX as u64)
        {
            return Err(ApiError::invalid("state and expected_version are required"));
        }
        let manage = self.authorize(call, &req.repo, Cap::IssueManage, None).await.is_ok();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::IssueWrite, None, move |repo, actor| {
            let old =
                repo.meta().read(|tx| tx.issue(req.number))?.ok_or_else(|| ApiError::not_found("issue not found"))?;
            if !manage && !super::owns(&old.author, actor) {
                return Err(ApiError::denied("only the issue author or an administrator can edit it"));
            }
            let result = repo.meta().write(|tx| {
                let mut row = found(tx, req.number)?;
                if row.revision != req.expected_version {
                    return Ok(None);
                }
                task(tx, &req.task_id)?;
                let kind = if row.state == req.state {
                    "updated"
                } else if req.state == "closed" {
                    "closed"
                } else {
                    "reopened"
                };
                row.title = req.title.trim().into();
                row.body = req.body.clone();
                row.state = req.state.clone();
                row.task_id = (!req.task_id.is_empty()).then_some(req.task_id.clone());
                if !tx.update_issue(&row, req.expected_version)? {
                    return Ok(None);
                }
                row = found(tx, req.number)?;
                record(tx, &row, actor, kind, "")?;
                Ok(Some(wire(row)))
            })?;
            result.ok_or_else(|| {
                ApiError::new(ErrorCode::FailedPrecondition, "this issue changed; reload it before saving")
            })
        })
        .await
    }
    /// Adds an immutable, idempotent discussion entry.
    pub async fn add_issue_comment(
        &self,
        call: &Call,
        req: v1::AddIssueCommentRequest,
    ) -> ApiResult<v1::IssueActivity> {
        number(req.number)?;
        if req.body.trim().is_empty() || req.body.len() > 16384 || req.body.contains('\0') {
            return Err(ApiError::invalid("comment must be non-empty and at most 16 KiB"));
        }
        let idem = self.idem();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::IssueWrite, None, move |repo, actor| {
            if repo.meta().read(|tx| tx.issue(req.number))?.is_none() {
                return Err(ApiError::not_found("issue not found"));
            }
            let fingerprint =
                serde_json::to_string(&(req.number, &req.body, actor.as_str())).map_err(ApiError::internal)?;
            keyed(repo, idem, "issue.comment", &req.idempotency_key, &fingerprint, |claim| {
                Ok(repo.meta().write(|tx| {
                    let row = found(tx, req.number)?;
                    let event = record(tx, &row, actor, "commented", &req.body)?;
                    claim.record(tx, &event)?;
                    Ok(event)
                })?)
            })
        })
        .await
    }
    /// Reads a bounded page of issue discussion and state changes.
    pub async fn list_issue_activity(
        &self,
        call: &Call,
        req: v1::ListIssueActivityRequest,
    ) -> ApiResult<v1::ListIssueActivityResponse> {
        number(req.number)?;
        let size = page_size(req.page_size).min(100);
        let cursor_scope = format!("issue-activity-{}-{}", req.repo.replace(':', "_"), req.number);
        let after = decode_offset(&cursor_scope, &req.page_token)? as u64;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::IssueRead, None, move |repo, _| {
            if repo.meta().read(|tx| tx.issue(req.number))?.is_none() {
                return Err(ApiError::not_found("issue not found"));
            }
            let mut rows = repo.meta().read(|tx| tx.issue_activity(req.number, after, size + 1))?;
            let more = rows.len() > size;
            rows.truncate(size);
            let next =
                if more { encode_offset(&cursor_scope, rows.last().unwrap().id as usize) } else { String::new() };
            Ok(v1::ListIssueActivityResponse {
                activity: rows.into_iter().map(activity).collect(),
                next_page_token: next,
            })
        })
        .await
    }
}
