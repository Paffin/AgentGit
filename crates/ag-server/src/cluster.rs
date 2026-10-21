//! Signalling between replicas of one installation (ADR-0007).
//!
//! The durable state is always the source of truth — the per-repository audit log behind the event feed, the job
//! table, the role bindings in the control database. This module only makes replicas *notice* changes sooner than
//! their polling would:
//!
//! * a replica that changed something publishes a short notice (`events`, `jobs`, `bindings`) with PostgreSQL
//!   `NOTIFY` on the control database;
//! * every replica holds one `LISTEN` connection and turns notices into wake-ups of its local waiters
//!   (SSE subscribers, head long-polls, job claimers) and into an invalidation of its binding cache.
//!
//! `NOTIFY` is not durable: a notice is lost if the listener is reconnecting, and PostgreSQL may drop one under
//! extreme load. That costs latency, never correctness — every consumer polls anyway (events 500 ms, jobs 5 s,
//! bindings by TTL) — and a listener that (re)connects reports [`Signal::Resync`], on which everything is
//! refreshed as if each notice had been missed. With SQLite as the control database there is nothing to listen to:
//! the bus is inert and a single process wakes its own waiters directly.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use prometheus::{IntCounter, IntCounterVec, IntGauge};
use tokio::sync::Notify;

use crate::control::{Control, ControlKind};

/// The PostgreSQL notification channel all replicas share.
pub const CHANNEL: &str = "ag_cluster";

/// Something another replica (or a reconnect) wants this one to know.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
    /// A repository's audit log / stream heads changed.
    Events(String),
    /// A repository may have claimable jobs.
    Jobs(String),
    /// Role bindings changed.
    Bindings,
    /// Notices may have been missed: refresh everything.
    Resync,
}

impl Signal {
    fn encode(&self, origin: &str) -> Option<String> {
        Some(match self {
            Signal::Events(r) => format!("{origin}|e|{r}"),
            Signal::Jobs(r) => format!("{origin}|j|{r}"),
            Signal::Bindings => format!("{origin}|b|"),
            Signal::Resync => return None,
        })
    }

    /// Parses a payload; returns the origin too, so a replica can ignore its own notices.
    fn decode(payload: &str) -> Option<(&str, Signal)> {
        let mut it = payload.splitn(3, '|');
        let (origin, kind, arg) = (it.next()?, it.next()?, it.next()?);
        Some((
            origin,
            match kind {
                "e" if !arg.is_empty() => Signal::Events(arg.to_string()),
                "j" if !arg.is_empty() => Signal::Jobs(arg.to_string()),
                "b" => Signal::Bindings,
                _ => return None,
            },
        ))
    }

    fn kind(&self) -> &'static str {
        match self {
            Signal::Events(_) => "events",
            Signal::Jobs(_) => "jobs",
            Signal::Bindings => "bindings",
            Signal::Resync => "resync",
        }
    }
}

/// What waits inside this process, and how a [`Signal`] wakes it.
#[derive(Default)]
pub struct Local {
    /// Bumped whenever the cached role bindings must be reloaded.
    pub bindings_epoch: AtomicU64,
    notifiers: Mutex<HashMap<String, Arc<Notify>>>,
    /// Repositories that may have queued jobs (a hint, rebuilt by scanning).
    pub job_repos: Mutex<BTreeSet<String>>,
    /// Wakes workers long-polling for jobs.
    pub job_notify: Notify,
    /// Set to make the next claimer rescan for jobs at once.
    pub job_rescan_due: AtomicBool,
}

impl Local {
    /// The wake-up handle of a repository's event feed.
    pub fn notifier(&self, repo: &str) -> Arc<Notify> {
        self.notifiers.lock().unwrap_or_else(|e| e.into_inner()).entry(repo.to_string()).or_default().clone()
    }

    /// Wakes the waiters of `repo` in this process.
    pub fn wake_repo(&self, repo: &str) {
        if let Some(n) = self.notifiers.lock().unwrap_or_else(|e| e.into_inner()).get(repo) {
            n.notify_waiters();
        }
    }

    /// Remembers that `repo` may have jobs and wakes claimers.
    pub fn note_jobs(&self, repo: &str) {
        self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).insert(repo.to_string());
        self.job_notify.notify_waiters();
    }

    /// Makes cached bindings stale.
    pub fn invalidate_bindings(&self) {
        self.bindings_epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Applies a signal to this process only.
    pub fn apply(&self, s: &Signal) {
        match s {
            Signal::Events(r) => self.wake_repo(r),
            Signal::Jobs(r) => self.note_jobs(r),
            Signal::Bindings => self.invalidate_bindings(),
            Signal::Resync => {
                self.invalidate_bindings();
                let all: Vec<_> = self.notifiers.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
                for n in all {
                    n.notify_waiters();
                }
                self.job_rescan_due.store(true, Ordering::Release);
                self.job_notify.notify_waiters();
            }
        }
    }
}

/// Metrics the bus maintains.
#[derive(Clone)]
pub struct BusMetrics {
    /// Notices by direction (`sent`, `received`) and kind.
    pub notices: IntCounterVec,
    /// 1 while the `LISTEN` connection is up.
    pub listener_up: IntGauge,
    /// Times a (re)connected listener forced a full refresh.
    pub resyncs: IntCounter,
}

/// The sending and receiving side of the bus. Dropping it stops both threads.
pub struct Bus {
    out: Option<mpsc::Sender<Signal>>,
    stop: Arc<AtomicBool>,
}

impl Bus {
    /// Starts the bus. On SQLite nothing is started.
    pub fn start(control: Arc<Control>, origin: String, local: Arc<Local>, metrics: BusMetrics) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if control.kind() == ControlKind::Sqlite {
            return Self { out: None, stop };
        }
        let (tx, rx) = mpsc::channel::<Signal>();
        {
            let (control, origin, metrics) = (control.clone(), origin.clone(), metrics.clone());
            std::thread::Builder::new()
                .name("ag-cluster-send".into())
                .spawn(move || publisher(&control, &origin, rx, &metrics))
                .expect("spawn thread");
        }
        {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("ag-cluster-listen".into())
                .spawn(move || listener(&control, &origin, &local, &metrics, &stop))
                .expect("spawn thread");
        }
        Self { out: Some(tx), stop }
    }

    /// Tells the other replicas. Never blocks and never fails: a notice that cannot be delivered is only a latency cost.
    pub fn publish(&self, s: Signal) {
        if let Some(tx) = &self.out {
            let _ = tx.send(s);
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn publisher(control: &Control, origin: &str, rx: mpsc::Receiver<Signal>, metrics: &BusMetrics) {
    while let Ok(first) = rx.recv() {
        // Many requests in a burst ask for the same wake-up: send each distinct notice once.
        let mut batch = vec![first];
        while let Ok(s) = rx.try_recv() {
            batch.push(s);
        }
        let mut seen = HashSet::new();
        for s in batch.into_iter().filter(|s| seen.insert(s.clone())) {
            let Some(payload) = s.encode(origin) else { continue };
            match control.notify(CHANNEL, &payload) {
                Ok(()) => metrics.notices.with_label_values(&["sent", s.kind()]).inc(),
                Err(e) => tracing::debug!(error = %e, "cluster notice not sent"),
            }
        }
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total && !stop.load(Ordering::Acquire) {
        std::thread::sleep(step);
        waited += step;
    }
}

fn listener(control: &Control, origin: &str, local: &Local, metrics: &BusMetrics, stop: &AtomicBool) {
    let mut backoff = Duration::from_millis(500);
    while !stop.load(Ordering::Acquire) {
        match control.listen(CHANNEL) {
            Ok(Some(mut session)) => {
                backoff = Duration::from_millis(500);
                metrics.listener_up.set(1);
                metrics.resyncs.inc();
                // Anything that happened while we were not listening is unknown: refresh everything.
                local.apply(&Signal::Resync);
                tracing::info!("listening for cluster notices");
                while !stop.load(Ordering::Acquire) {
                    match session.wait(Duration::from_secs(1)) {
                        Ok(payloads) => {
                            for p in payloads {
                                match Signal::decode(&p) {
                                    Some((from, _)) if from == origin => {} // our own: already applied locally
                                    Some((_, s)) => {
                                        metrics.notices.with_label_values(&["received", s.kind()]).inc();
                                        local.apply(&s);
                                    }
                                    None => tracing::debug!("ignoring an unrecognized cluster notice"),
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "the cluster notice connection was lost; reconnecting");
                            break;
                        }
                    }
                }
                metrics.listener_up.set(0);
            }
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for cluster notices; retrying");
                sleep_unless_stopped(stop, backoff);
                backoff = (backoff * 2).min(Duration::from_secs(10));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ag_storage::SystemClock;

    use super::*;
    use crate::control::PostgresTarget;

    #[test]
    fn signals_round_trip_and_garbage_is_ignored() {
        for s in [Signal::Events("acme/web".into()), Signal::Jobs("a/b".into()), Signal::Bindings] {
            let wire = s.encode("pod-1").unwrap();
            assert_eq!(Signal::decode(&wire), Some(("pod-1", s)));
        }
        assert!(Signal::Resync.encode("x").is_none());
        for bad in ["", "x", "x|e|", "x|z|y", "x|j|"] {
            assert!(Signal::decode(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn local_signals_wake_waiters() {
        let local = Local::default();
        let n = local.notifier("acme/web");
        let notified = n.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        local.apply(&Signal::Events("acme/web".into()));
        tokio::time::timeout(Duration::from_secs(1), notified).await.expect("woken");
        local.apply(&Signal::Jobs("acme/web".into()));
        assert!(local.job_repos.lock().unwrap().contains("acme/web"));
        let before = local.bindings_epoch.load(Ordering::Acquire);
        local.apply(&Signal::Bindings);
        assert_eq!(local.bindings_epoch.load(Ordering::Acquire), before + 1);
        local.apply(&Signal::Resync);
        assert!(local.job_rescan_due.load(Ordering::Acquire));
    }

    fn metrics() -> BusMetrics {
        BusMetrics {
            notices: IntCounterVec::new(prometheus::Opts::new("n", "n"), &["direction", "kind"]).unwrap(),
            listener_up: IntGauge::new("u", "u").unwrap(),
            resyncs: IntCounter::new("r", "r").unwrap(),
        }
    }

    #[test]
    fn a_notice_on_one_replica_wakes_the_other_through_postgres() {
        let Some(url) = ag_testkit::require_here(
            "postgres",
            "AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)",
            ag_storage::testing::postgres_url(),
        ) else {
            return;
        };
        let schema = ag_storage::testing::unique("agbus");
        let target = PostgresTarget {
            url: url.clone(),
            schema: schema.clone(),
            pool_size: 4,
            startup_timeout: Duration::from_secs(5),
        };
        let control = Arc::new(Control::open_postgres(&target, Arc::new(SystemClock)).unwrap());
        let (la, lb) = (Arc::new(Local::default()), Arc::new(Local::default()));
        let (ma, mb) = (metrics(), metrics());
        let a = Bus::start(control.clone(), "a".into(), la.clone(), ma.clone());
        let _b = Bus::start(control.clone(), "b".into(), lb.clone(), mb.clone());
        let until = |f: &dyn Fn() -> bool| {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(10) {
                if f() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(until(&|| mb.listener_up.get() == 1 && ma.listener_up.get() == 1), "listeners connect");
        let epoch = lb.bindings_epoch.load(Ordering::Acquire);
        a.publish(Signal::Bindings);
        a.publish(Signal::Jobs("acme/web".into()));
        assert!(until(&|| lb.job_repos.lock().unwrap().contains("acme/web")), "replica b learns of the job");
        assert!(until(&|| lb.bindings_epoch.load(Ordering::Acquire) > epoch), "replica b drops its bindings");
        // The publisher does not wake itself through the database.
        assert!(!la.job_repos.lock().unwrap().contains("acme/web"));
        assert_eq!(mb.notices.with_label_values(&["received", "jobs"]).get(), 1);
        drop(a);
        drop(_b);
        let _ = ag_storage::meta::drop_schema(&url, &schema);
    }
}
