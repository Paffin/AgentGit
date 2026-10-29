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

/// How many audit rows one read examines at a time while looking for events that match the topics.
const SCAN_PAGE: usize = 1024;

/// A page of events and the newest sequence number at the time it was read.
pub struct EventBatch {
    /// Events with `seq` greater than the cursor, oldest first. Fewer than the limit means there are no more
    /// matching events up to `latest_seq`; a full page may be followed by more.
    pub events: Vec<v1::Event>,
    /// Highest sequence number in the log.
    pub latest_seq: u64,
    /// Highest sequence number this read examined. Every event of the requested topics at or below it is in
    /// `events` (up to the limit), so a follower may safely resume after it - and must not resume after
    /// anything higher, which would skip events it never looked at.
    pub scanned_seq: u64,
}

pub(super) fn to_event(repo: &str, e: &AuditEntry) -> v1::Event {
    let details: serde_json::Value = serde_json::from_str(&e.details).unwrap_or(serde_json::Value::Null);
    v1::Event {
        seq: e.seq,
        topic: e.action.clone(),
        repo: repo.to_string(),
        timestamp_ms: e.timestamp.as_millis(),
        payload_json: json!({"actor": e.actor, "target": e.target, "details": details}).to_string(),
    }
}

pub(super) fn matches_topics(topics: &[String], topic: &str) -> bool {
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
            // The topic filter runs while paging through the log, until `limit` events matched or the log ended:
            // a run of unrelated entries (job claims, lease renewals...) never hides a matching event behind it.
            r.meta()
                .read(|tx| {
                    let latest_seq = tx.audit_latest_seq()?;
                    let mut events: Vec<v1::Event> = Vec::new();
                    let mut cursor = after_seq;
                    let mut scanned_seq = after_seq;
                    'scan: loop {
                        let page = tx.audit_after(cursor, SCAN_PAGE)?;
                        for e in &page {
                            scanned_seq = e.seq;
                            if matches_topics(&topics, &e.action) {
                                events.push(to_event(&name, e));
                                if events.len() >= limit {
                                    break 'scan;
                                }
                            }
                        }
                        match page.last() {
                            Some(last) if page.len() == SCAN_PAGE => cursor = last.seq,
                            _ => break,
                        }
                    }
                    Ok(EventBatch { events, latest_seq, scanned_seq })
                })
                .map_err(Into::into)
        })
        .await
    }

    /// One-shot replay.
    pub async fn replay(&self, call: &Call, req: v1::ReplayRequest) -> ApiResult<v1::ReplayResponse> {
        let limit = if req.limit == 0 { 500 } else { req.limit.min(5000) } as usize;
        let b = self.events_after(call, require(&req.repo, "repo")?, &req.topics, req.after_seq, limit).await?;
        // A page with fewer than `limit` events is the end of the matching events up to `latest_seq`.
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
        // Fail early (and authorize) before returning a stream. Reading after the highest possible sequence number
        // returns no events and costs nothing; it yields the log's current end.
        let first = self.events_after(&call, &repo, &req.topics, u64::MAX, 1).await?;
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
            live_after: first.latest_seq,
            buffer: VecDeque::new(),
            last_sent: Instant::now(),
            last_credential_check: Instant::now(),
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
                    if st.call.principal.scope.bound_run().is_some()
                        && let Err(error) = st.credential_still_valid().await
                    {
                        st.done = true;
                        return Some((Err(error), st));
                    }
                    st.last_sent = Instant::now();
                    if ev.seq > st.live_after {
                        // Only events recorded after the subscription began measure the live path (not a replay).
                        let lag_ms = st.app.clock.now().as_millis().saturating_sub(ev.timestamp_ms).max(0);
                        st.app.metrics.event_lag.observe(lag_ms as f64 / 1000.0);
                    }
                    return Some((Ok(ev), st));
                }
                // An idle stream looks at its credential every few seconds; a batch about to be delivered is
                // preceded by a check of its own (below), so nothing is sent after a revocation the server can see.
                if st.last_credential_check.elapsed() >= CREDENTIAL_RECHECK
                    && let Err(e) = st.credential_still_valid().await
                {
                    st.done = true;
                    return Some((Err(e), st));
                }
                // Register for wake-ups before reading so nothing is missed in between.
                let notify = st.app.notifier(&st.repo);
                let notified = notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                match st.app.events_after(&st.call, &st.repo, &st.topics, st.cursor, 256).await {
                    Ok(batch) => {
                        if !batch.events.is_empty()
                            && let Err(e) = st.credential_still_valid().await
                        {
                            st.done = true;
                            return Some((Err(e), st));
                        }
                        // Resume after what was actually examined - never past it: events beyond a full page of
                        // matches, or beyond what the scan reached, have not been looked at.
                        st.cursor = st.cursor.max(batch.scanned_seq);
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

pub(super) struct GaugeGuard(pub(super) App);

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.0.metrics.subscriptions.dec();
    }
}

/// How often an open event stream checks that its credential is still valid.
const CREDENTIAL_RECHECK: Duration = Duration::from_secs(5);

impl FeedState {
    /// Re-validates the credential the stream was opened with: not revoked (on any replica), not expired.
    async fn credential_still_valid(&mut self) -> ApiResult<()> {
        self.last_credential_check = Instant::now();
        let expires = self.call.principal.expires_at_ms;
        if expires > 0 && self.app.clock.now().as_millis() >= expires {
            return Err(ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "the credential expired"));
        }
        self.app.auth.still_active(&self.call.principal).await?;
        // Recheck the effective run after reading a batch: it may have ended during that read.
        self.app.ensure_bound_run_alive(&self.call.principal).await?;
        self.app.authorize(&self.call, &self.repo, Cap::EventsRead, None).await
    }
}

struct FeedState {
    app: App,
    call: Call,
    repo: String,
    topics: Vec<String>,
    cursor: u64,
    /// Events with a higher `seq` were recorded after the subscription began.
    live_after: u64,
    buffer: VecDeque<v1::Event>,
    last_sent: Instant,
    last_credential_check: Instant,
    heartbeat: Duration,
    done: bool,
    shutdown: tokio::sync::watch::Receiver<bool>,
    _gauge: GaugeGuard,
}
