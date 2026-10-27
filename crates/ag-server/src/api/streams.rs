//! Streams: list, create, read head (with long-poll), compare-and-swap, reflog.

use std::time::{Duration, Instant};

use ag_protocol::v1;
use ag_storage::meta::StreamState;
use ag_types::{ChangesetId, StreamName};

use super::{convert, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

const MAX_WAIT_MS: u32 = 30_000;

impl App {
    /// Lists the streams of a repository.
    pub async fn list_streams(&self, call: &Call, req: v1::ListStreamsRequest) -> ApiResult<v1::ListStreamsResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::StreamRead, None, |repo, _| {
            let streams = repo.meta().read(|tx| {
                let mut out = Vec::new();
                for s in tx.list_streams()? {
                    let pv = tx.policy_current(s.name.as_str())?.map(|p| p.version).unwrap_or(0);
                    out.push(convert::stream(&s, pv));
                }
                Ok(out)
            })?;
            Ok(v1::ListStreamsResponse { streams })
        })
        .await
    }

    /// Creates a stream, optionally at an existing revision.
    pub async fn create_stream(&self, call: &Call, req: v1::CreateStreamRequest) -> ApiResult<v1::Stream> {
        let name = StreamName::new(&req.name).map_err(|e| ApiError::invalid(e.to_string()))?;
        let max_streams = self.repo_row(&req.repo).await?.map(|r| r.quota.max_streams).unwrap_or(0);
        let from_rev = req.from_rev.clone();
        let stream_name = name.to_string();
        self.with_repo_mut(
            call,
            require(&req.repo, "repo")?,
            Cap::StreamCreate,
            Some(&stream_name),
            move |repo, actor| {
                let from = if from_rev.is_empty() { None } else { Some(repo.resolve_rev(&from_rev)?) };
                // The limit is counted inside the transaction that inserts the stream.
                let cap = (max_streams > 0).then_some(u64::from(max_streams));
                let row = repo.create_stream_capped(&name, from.as_ref(), actor, cap).map_err(|e| match e {
                    ag_integrate::Error::Meta(ag_storage::meta::MetaError::AlreadyExists(m)) => {
                        ApiError::new(ag_protocol::ErrorCode::AlreadyExists, m)
                    }
                    other => other.into(),
                })?;
                Ok(convert::stream(&row, 0))
            },
        )
        .await
    }

    /// Reads a stream head. With `wait_version_gt` and `wait_ms` it waits
    /// (up to 30 s) until the stream version exceeds the given value.
    pub async fn read_head(&self, call: &Call, req: v1::ReadHeadRequest) -> ApiResult<v1::Stream> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let stream = require(&req.stream, "stream")?.to_string();
        let wait = Duration::from_millis(u64::from(req.wait_ms.min(MAX_WAIT_MS)));
        let deadline = Instant::now() + wait;
        let notify = self.notifier(&repo_name);
        loop {
            // Register for wake-ups *before* reading so an update in between is not missed.
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let s = stream.clone();
            let row = self
                .with_repo(call, &repo_name, Cap::StreamRead, Some(&stream), move |repo, _| {
                    let row = repo
                        .meta()
                        .read(|tx| tx.stream(&s))?
                        .ok_or_else(|| ApiError::not_found(format!("stream '{s}' not found")))?;
                    let pv = repo.meta().read(|tx| tx.policy_current(&s))?.map(|p| p.version).unwrap_or(0);
                    Ok(convert::stream(&row, pv))
                })
                .await?;
            if req.wait_ms == 0 || row.version > req.wait_version_gt || Instant::now() >= deadline {
                return Ok(row);
            }
            let remaining = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(500));
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Administrative compare-and-swap of a stream head.
    pub async fn compare_and_swap(&self, call: &Call, req: v1::CompareAndSwapRequest) -> ApiResult<v1::Stream> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let stream = require(&req.stream, "stream")?.to_string();
        let new_head: ChangesetId = parse_id(&req.new_head, "changeset id")?;
        let expected_head: Option<ChangesetId> =
            if req.expected_head.is_empty() { None } else { Some(parse_id(&req.expected_head, "changeset id")?) };
        if req.force {
            self.authorize(call, &repo_name, Cap::StreamForce, Some(&stream)).await?;
        }
        let expected = StreamState { head: expected_head, version: req.expected_version };
        let (reason, force) = (req.reason.clone(), req.force);
        let s = stream.clone();
        self.with_repo_mut(call, &repo_name, Cap::StreamCas, Some(&stream), move |repo, actor| {
            repo.stream_cas(&s, &expected, &new_head, actor, &reason, force)?;
            let row = repo.meta().read(|tx| tx.stream(&s))?.ok_or_else(|| ApiError::not_found("stream vanished"))?;
            let pv = repo.meta().read(|tx| tx.policy_current(&s))?.map(|p| p.version).unwrap_or(0);
            Ok(convert::stream(&row, pv))
        })
        .await
    }

    /// The reflog of a stream, newest first.
    pub async fn reflog(&self, call: &Call, req: v1::ReflogRequest) -> ApiResult<v1::ReflogResponse> {
        let stream = require(&req.stream, "stream")?.to_string();
        let limit = if req.limit == 0 { 100 } else { req.limit.min(1000) } as usize;
        let s = stream.clone();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::StreamRead, Some(&stream), move |repo, _| {
            let entries = repo.meta().read(|tx| tx.reflog(&s, limit))?;
            Ok(v1::ReflogResponse { entries: entries.iter().map(convert::reflog).collect() })
        })
        .await
    }
}
