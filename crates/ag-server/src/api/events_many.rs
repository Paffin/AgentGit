//! Bounded audit replay across explicit repositories, with independent cursors.
use super::events::{GaugeGuard, matches_topics, to_event};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};
use ag_protocol::v1;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_REPOS: usize = 30;
const MAX_CURSOR: usize = 16 * 1024;
const SCAN_BUDGET: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    v: u8,
    repos: Vec<String>,
    topics: Vec<String>,
    seq: Vec<u64>,
    next: usize,
}
impl Cursor {
    fn encoded(&self) -> ApiResult<String> {
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).map_err(ApiError::internal)?);
        if encoded.len() > MAX_CURSOR {
            return Err(ApiError::invalid("event cursor exceeds 16 KiB"));
        }
        Ok(encoded)
    }
}
fn normalized(mut repos: Vec<String>, mut topics: Vec<String>) -> ApiResult<(Vec<String>, Vec<String>)> {
    if repos.is_empty()
        || repos.len() > MAX_REPOS
        || repos.iter().any(|repo| repo.is_empty() || repo.len() > 256 || repo.contains(['*', '?', ',']))
    {
        return Err(ApiError::invalid("supply 1 to 30 explicit repository names"));
    }
    repos.sort();
    repos.dedup();
    if topics.len() > 32 || topics.iter().any(|topic| topic.is_empty() || topic.len() > 128) {
        return Err(ApiError::invalid("supply at most 32 nonempty topic prefixes of 128 bytes"));
    }
    topics.sort();
    topics.dedup();
    Ok((repos, topics))
}
fn decode(value: &str, repos: &[String], topics: &[String]) -> ApiResult<Cursor> {
    if value.len() > MAX_CURSOR {
        return Err(ApiError::invalid("event cursor exceeds 16 KiB"));
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| ApiError::invalid("invalid event cursor"))?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| ApiError::invalid("invalid event cursor"))?;
    if cursor.v != 1
        || cursor.repos != repos
        || cursor.topics != topics
        || cursor.seq.len() != repos.len()
        || cursor.next >= repos.len()
    {
        return Err(ApiError::invalid("event cursor does not match the requested repositories and topics"));
    }
    Ok(cursor)
}
impl App {
    async fn authorize_many(&self, call: &Call, repos: &[String]) -> ApiResult<()> {
        let expires = call.principal.expires_at_ms;
        if expires > 0 && self.clock.now().as_millis() >= expires {
            return Err(ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "the credential expired"));
        }
        self.auth.still_active(&call.principal).await?;
        self.ensure_bound_run_alive(&call.principal).await?;
        for repo in repos {
            self.authorize(call, repo, Cap::EventsRead, None).await?;
        }
        Ok(())
    }
    async fn many_page(
        &self,
        call: &Call,
        repo: &str,
        topics: &[String],
        after: u64,
        limit: usize,
        scan: usize,
    ) -> ApiResult<(Vec<v1::Event>, u64, u64)> {
        let (name, topics) = (repo.to_string(), topics.to_vec());
        self.with_repo(call, repo, Cap::EventsRead, None, move |r, _| {
            r.meta()
                .read(|tx| {
                    let latest = tx.audit_latest_seq()?;
                    let rows = tx.audit_after(after, scan)?;
                    let mut scanned = after;
                    let mut events = Vec::new();
                    for row in rows {
                        scanned = row.seq;
                        if matches_topics(&topics, &row.action) {
                            events.push(to_event(&name, &row));
                        }
                        if events.len() >= limit {
                            break;
                        }
                    }
                    Ok((events, scanned, latest))
                })
                .map_err(Into::into)
        })
        .await
    }
    /// Checkpoint from now by default; explicit historical replay uses `from_start`.
    pub async fn replay_many(&self, call: &Call, req: v1::ReplayManyRequest) -> ApiResult<v1::ReplayManyResponse> {
        let (repos, topics) = normalized(req.repos, req.topics)?;
        if req.from_start && !req.cursor.is_empty() {
            return Err(ApiError::invalid("from_start cannot be combined with cursor"));
        }
        self.authorize_many(call, &repos).await?;
        let mut cursor = if req.cursor.is_empty() {
            Cursor { v: 1, seq: vec![0; repos.len()], next: 0, repos: repos.clone(), topics: topics.clone() }
        } else {
            decode(&req.cursor, &repos, &topics)?
        };
        if req.cursor.is_empty() && !req.from_start {
            for (i, repo) in repos.iter().enumerate() {
                cursor.seq[i] = self.many_page(call, repo, &topics, u64::MAX, 1, 1).await?.2;
            }
            self.authorize_many(call, &repos).await?;
            return Ok(v1::ReplayManyResponse { events: vec![], cursor: cursor.encoded()?, has_more: false });
        }
        let limit = if req.limit == 0 { 256 } else { req.limit.min(1000) } as usize;
        let per_repo = limit.div_ceil(repos.len());
        let scan = SCAN_BUDGET / repos.len();
        let start = cursor.next;
        let mut events = Vec::new();
        let mut has_more = false;
        for offset in 0..repos.len() {
            let i = (start + offset) % repos.len();
            if events.len() == limit {
                has_more = true;
                break;
            }
            let (batch, scanned, latest) = self
                .many_page(call, &repos[i], &topics, cursor.seq[i], per_repo.min(limit - events.len()), scan)
                .await?;
            for event in batch {
                cursor.seq[i] = event.seq;
                cursor.next = (i + 1) % repos.len();
                events.push(v1::EventEnvelope { cursor: cursor.encoded()?, event: Some(event) });
            }
            cursor.seq[i] = cursor.seq[i].max(scanned);
            cursor.next = (i + 1) % repos.len();
            has_more |= scanned < latest;
        }
        self.authorize_many(call, &repos).await?;
        Ok(v1::ReplayManyResponse { events, cursor: cursor.encoded()?, has_more })
    }
    /// Replays the cursor then follows. Each emitted envelope remains independently resumable.
    pub async fn subscribe_many(
        self,
        call: Call,
        req: v1::SubscribeManyRequest,
    ) -> ApiResult<impl Stream<Item = ApiResult<v1::EventEnvelope>> + Send + 'static> {
        let (repos, topics) = normalized(req.repos, req.topics)?;
        let first = self
            .replay_many(
                &call,
                v1::ReplayManyRequest {
                    repos: repos.clone(),
                    topics: topics.clone(),
                    cursor: req.cursor,
                    limit: 256,
                    from_start: false,
                },
            )
            .await?;
        self.metrics.subscriptions.inc();
        let heartbeat =
            Duration::from_millis(u64::from(if req.heartbeat_ms == 0 { 15000 } else { req.heartbeat_ms.max(1000) }));
        let mut buffer: VecDeque<_> = first.events.into();
        buffer.push_back(v1::EventEnvelope { cursor: first.cursor.clone(), event: None });
        let state = ManyState {
            app: self.clone(),
            call,
            repos,
            topics,
            cursor: first.cursor,
            buffer,
            heartbeat,
            last_sent: Instant::now(),
            done: false,
            shutdown: self.shutdown_signal(),
            _gauge: GaugeGuard(self),
        };
        Ok(futures_util::stream::unfold(state, |mut st| async move {
            loop {
                if st.done || *st.shutdown.borrow() {
                    return None;
                }
                if let Some(event) = st.buffer.pop_front() {
                    if let Err(error) = st.app.authorize_many(&st.call, &st.repos).await {
                        st.done = true;
                        return Some((Err(error), st));
                    }
                    st.last_sent = Instant::now();
                    return Some((Ok(event), st));
                }
                let req = v1::ReplayManyRequest {
                    repos: st.repos.clone(),
                    topics: st.topics.clone(),
                    cursor: st.cursor.clone(),
                    limit: 256,
                    from_start: false,
                };
                match st.app.replay_many(&st.call, req).await {
                    Ok(page) => {
                        st.buffer.extend(page.events);
                        if page.cursor != st.cursor {
                            st.buffer.push_back(v1::EventEnvelope { cursor: page.cursor.clone(), event: None });
                        }
                        st.cursor = page.cursor;
                        if !st.buffer.is_empty() {
                            continue;
                        }
                        if st.last_sent.elapsed() >= st.heartbeat {
                            st.buffer.push_back(v1::EventEnvelope { cursor: st.cursor.clone(), event: None });
                            continue;
                        }
                    }
                    Err(error) => {
                        st.done = true;
                        return Some((Err(error), st));
                    }
                }
                let mut shutdown = st.shutdown.clone();
                tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=shutdown.changed()=>{}}
            }
        }))
    }
}
struct ManyState {
    app: App,
    call: Call,
    repos: Vec<String>,
    topics: Vec<String>,
    cursor: String,
    buffer: VecDeque<v1::EventEnvelope>,
    heartbeat: Duration,
    last_sent: Instant,
    done: bool,
    shutdown: tokio::sync::watch::Receiver<bool>,
    _gauge: GaugeGuard,
}
