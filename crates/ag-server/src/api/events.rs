//! Events: a durable, ordered feed built on the repository's audit log.
//!
//! Every state change in a repository is recorded in its hash-chained audit
//! log in the same transaction as the change itself. The event feed is a view
//! of that log: `seq` is the audit sequence number (strictly increasing, never
//! reused), `topic` is the audit action (`stream.integrate`,
//! `proposal.create`, ...), and the payload carries actor, target and details.
//! A subscriber that was away resumes with `after_seq` and loses nothing.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ag_protocol::v1;
use ag_storage::meta::AuditEntry;
use futures_util::Stream;
use serde_json::json;

use super::require;
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

/// A page of events and the newest sequence number at the time it was read.
pub struct EventBatch {
    /// Events with `seq` greater than the cursor, oldest first.
    pub events: Vec<v1::Event>,
    /// Highest sequence number in the log.
    pub latest_seq: u64,
}

fn to_event(repo: &str, e: &AuditEntry) -> v1::Event {
    let details: serde_json::Value = serde_json::from_str(&e.details).unwrap_or(serde_json::Value::Null);
    v1::Event {
        seq: e.seq,
        topic: e.action.clone(),
        repo: repo.to_string(),
        timestamp_ms: e.timestamp.as_millis(),
        payload_json: json!({"actor": e.actor, "target": e.target, "details": details}).to_string(),
    }
}

fn matches_topics(topics: &[String], topic: &str) -> bool {
    topics.is_empty() || topics.iter().any(|t| topic == t || topic.starts_with(&format!("{t}.")) || t == "*")
}

impl App {
    /// Reads events after `after_seq` (oldest first).
    pub async fn events_after(
        &self,
        call: &Call,
        repo: &str,
        topics: &[String],
        after_seq: u64,
        limit: usize,
    ) -> ApiResult<EventBatch> {
        let topics = topics.to_vec();
        let name = repo.to_string();
        self.with_repo(call, repo, Cap::EventsRead, None, move |r, _| {
            let (entries, latest) = r.meta().read(|tx| {
                let latest = tx.audit_latest_seq()?;
                Ok((tx.audit_after(after_seq, limit.saturating_mul(4).max(limit))?, latest))
            })?;
            let events: Vec<v1::Event> = entries
                .iter()
                .filter(|e| matches_topics(&topics, &e.action))
                .take(limit)
                .map(|e| to_event(&name, e))
                .collect();
            Ok(EventBatch { events, latest_seq: latest })
        })
        .await
    }

    /// One-shot replay.
    pub async fn replay(&self, call: &Call, req: v1::ReplayRequest) -> ApiResult<v1::ReplayResponse> {
        let limit = if req.limit == 0 { 500 } else { req.limit.min(5000) } as usize;
        let b = self.events_after(call, require(&req.repo, "repo")?, &req.topics, req.after_seq, limit).await?;
        Ok(v1::ReplayResponse { events: b.events, latest_seq: b.latest_seq })
    }

    /// A live feed: replays from `after_seq`, then follows new events.
    /// Heartbeat events (`topic = "heartbeat"`, `seq = 0`) keep idle
    /// connections alive. The stream ends when the credential expires or
    /// access is revoked.
    pub async fn subscribe(
        self,
        call: Call,
        req: v1::SubscribeRequest,
    ) -> ApiResult<impl Stream<Item = ApiResult<v1::Event>> + Send + 'static> {
        let repo = require(&req.repo, "repo")?.to_string();
        // Fail early (and authorize) before returning a stream.
        let first = self.events_after(&call, &repo, &req.topics, 0, 1).await?;
        let start = if req.after_seq == u64::MAX { first.latest_seq } else { req.after_seq };
        let heartbeat =
            Duration::from_millis(u64::from(if req.heartbeat_ms == 0 { 15_000 } else { req.heartbeat_ms.max(1000) }));
        self.metrics.subscriptions.inc();
        let state = FeedState {
            app: self.clone(),
            call,
            repo,
            topics: req.topics,
            cursor: start,
            buffer: VecDeque::new(),
            last_sent: Instant::now(),
            heartbeat,
            done: false,
            shutdown: self.shutdown_signal(),
            _gauge: GaugeGuard(self.clone()),
        };
        Ok(futures_util::stream::unfold(state, |mut st| async move {
            loop {
                if st.done || *st.shutdown.borrow() {
                    return None;
                }
                if let Some(ev) = st.buffer.pop_front() {
                    st.last_sent = Instant::now();
                    return Some((Ok(ev), st));
                }
                if st.call.principal.expires_at_ms > 0
                    && st.app.clock.now().as_millis() >= st.call.principal.expires_at_ms
                {
                    st.done = true;
                    return Some((
                        Err(ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "the credential expired")),
                        st,
                    ));
                }
                // Register for wake-ups before reading so nothing is missed in between.
                let notify = st.app.notifier(&st.repo);
                let notified = notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                match st.app.events_after(&st.call, &st.repo, &st.topics, st.cursor, 256).await {
                    Ok(batch) => {
                        if let Some(last) = batch.events.last() {
                            st.cursor = last.seq;
                        } else if batch.latest_seq > st.cursor {
                            // Only filtered-out events arrived; skip past them.
                            st.cursor = batch.latest_seq;
                        }
                        st.buffer.extend(batch.events);
                        if !st.buffer.is_empty() {
                            continue;
                        }
                    }
                    Err(e) => {
                        st.done = true;
                        return Some((Err(e), st));
                    }
                }
                let until_heartbeat = st.heartbeat.saturating_sub(st.last_sent.elapsed());
                if until_heartbeat.is_zero() {
                    st.last_sent = Instant::now();
                    let hb = v1::Event {
                        seq: 0,
                        topic: "heartbeat".into(),
                        repo: st.repo.clone(),
                        timestamp_ms: st.app.clock.now().as_millis(),
                        payload_json: "{}".into(),
                    };
                    return Some((Ok(hb), st));
                }
                // Poll the log at least every 500 ms so changes made by other server processes show up too.
                let mut shutdown = st.shutdown.clone();
                tokio::select! {
                    _ = tokio::time::timeout(until_heartbeat.min(Duration::from_millis(500)), notified) => {}
                    _ = shutdown.changed() => {}
                }
            }
        }))
    }
}

struct GaugeGuard(App);

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.0.metrics.subscriptions.dec();
    }
}

struct FeedState {
    app: App,
    call: Call,
    repo: String,
    topics: Vec<String>,
    cursor: u64,
    buffer: VecDeque<v1::Event>,
    last_sent: Instant,
    heartbeat: Duration,
    done: bool,
    shutdown: tokio::sync::watch::Receiver<bool>,
    _gauge: GaugeGuard,
}
