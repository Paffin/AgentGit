//! History: changesets, logs, ancestor negotiation, merge bases.

use ag_changeset::Op;
use ag_dag::merge_bases;
use ag_protocol::v1;
use ag_types::ChangesetId;

use super::{paginate, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::ApiResult;

pub(crate) fn changeset_to_wire(
    repo: &ag_integrate::Repository,
    id: &ChangesetId,
    include_ops: bool,
) -> ApiResult<v1::Changeset> {
    let cs = repo.changeset(id)?;
    let (generation, published) = repo.meta().read(|tx| Ok((tx.generation(id)?, tx.is_ready(id)?)))?;
    let ops = if include_ops {
        cs.ops
            .iter()
            .map(|op| match op {
                Op::Put { path, content, file_type, executable } => v1::FileOp {
                    path: path.to_string(),
                    op: "put".into(),
                    blob_id: content.to_string(),
                    file_type: file_type.to_string(),
                    executable: *executable,
                },
                Op::Delete { path } => v1::FileOp { path: path.to_string(), op: "delete".into(), ..Default::default() },
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(v1::Changeset {
        id: id.to_string(),
        namespace: cs.namespace.to_string(),
        parents: cs.parents.iter().map(|p| p.to_string()).collect(),
        author: cs.author.to_string(),
        timestamp_ms: cs.timestamp.as_millis(),
        message: cs.message.clone(),
        ops,
        generation: generation.unwrap_or(0),
        published,
    })
}

impl App {
    /// One changeset (revision expressions are resolved).
    pub async fn get_changeset(&self, call: &Call, req: v1::GetChangesetRequest) -> ApiResult<v1::Changeset> {
        let rev = require(&req.rev, "rev")?.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::HistoryRead, None, move |repo, _| {
            let id = repo.resolve_rev(&rev)?;
            changeset_to_wire(repo, &id, req.include_ops)
        })
        .await
    }

    /// History from a revision, newest first, optionally limited to a path.
    pub async fn log(&self, call: &Call, req: v1::LogRequest) -> ApiResult<v1::LogResponse> {
        let rev = if req.rev.is_empty() { "@".to_string() } else { req.rev.clone() };
        let (path, size, token) = (req.path.clone(), req.page_size, req.page_token.clone());
        let offset = super::decode_offset("log", &token)?;
        let want = super::page_size(size);
        self.with_repo(call, require(&req.repo, "repo")?, Cap::HistoryRead, None, move |repo, _| {
            let head = repo.resolve_rev(&rev)?;
            let entries =
                repo.log(&head, offset + want + 1, if path.is_empty() { None } else { Some(path.as_str()) })?;
            let (page, next) = paginate("log", entries, size, &token)?;
            let mut out = Vec::with_capacity(page.len());
            for e in page {
                out.push(changeset_to_wire(repo, &e.id, false)?);
            }
            Ok(v1::LogResponse { changesets: out, next_page_token: next })
        })
        .await
    }

    /// Changesets reachable from `want` but not from `have`, parents first.
    pub async fn ancestors(&self, call: &Call, req: v1::AncestorsRequest) -> ApiResult<v1::AncestorsResponse> {
        let parse =
            |v: &[String]| v.iter().map(|s| parse_id::<ChangesetId>(s, "changeset id")).collect::<ApiResult<Vec<_>>>();
        let (want, have) = (parse(&req.want)?, parse(&req.have)?);
        if want.len() + have.len() > ag_protocol::limits::MAX_BATCH as usize {
            return Err(crate::error::ApiError::new(ag_protocol::ErrorCode::TooLarge, "too many want/have entries"));
        }
        let (size, token) = (req.page_size, req.page_token.clone());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::HistoryRead, None, move |repo, _| {
            let ids = repo.ancestors_between(&want, &have)?;
            let (page, next) = paginate("anc", ids, size, &token)?;
            Ok(v1::AncestorsResponse {
                changeset_ids: page.iter().map(|i| i.to_string()).collect(),
                next_page_token: next,
            })
        })
        .await
    }

    /// Merge bases of two revisions.
    pub async fn merge_base(&self, call: &Call, req: v1::MergeBaseRequest) -> ApiResult<v1::MergeBaseResponse> {
        let (a, b) = (require(&req.a, "a")?.to_string(), require(&req.b, "b")?.to_string());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::HistoryRead, None, move |repo, _| {
            let (a, b) = (repo.resolve_rev(&a)?, repo.resolve_rev(&b)?);
            let bases = merge_bases(&repo.graph(), &a, &b).map_err(ag_integrate::Error::from)?;
            Ok(v1::MergeBaseResponse { bases: bases.iter().map(|i| i.to_string()).collect() })
        })
        .await
    }

    /// Resolves a revision expression.
    pub async fn resolve_rev(&self, call: &Call, req: v1::ResolveRevRequest) -> ApiResult<v1::ResolveRevResponse> {
        let rev = require(&req.rev, "rev")?.to_string();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::HistoryRead, None, move |repo, _| {
            Ok(v1::ResolveRevResponse { changeset_id: repo.resolve_rev(&rev)?.to_string() })
        })
        .await
    }
}
