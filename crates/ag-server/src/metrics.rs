//! Prometheus metrics.

use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};

/// All metrics exported at `/metrics`.
pub struct Metrics {
    registry: Registry,
    /// Requests by transport, operation and result code.
    pub requests: IntCounterVec,
    /// Request latency by transport and operation.
    pub latency: HistogramVec,
    /// Requests currently being handled.
    pub inflight: IntGauge,
    /// Failed authentications by reason.
    pub auth_failures: IntCounterVec,
    /// Requests rejected by the rate limiter.
    pub rate_limited: IntCounter,
    /// Requests shed because the server was saturated.
    pub shed: IntCounter,
    /// Integrations by outcome status.
    pub integrations: IntCounterVec,
    /// Object bytes accepted from clients.
    pub object_bytes_in: IntCounter,
    /// Object bytes sent to clients.
    pub object_bytes_out: IntCounter,
    /// Repositories currently open.
    pub open_repos: IntGauge,
    /// Open event subscriptions.
    pub subscriptions: IntGauge,
    /// Execution-plane job events since start: enqueued, claimed, succeeded, retried, failed.
    pub job_events: IntCounterVec,
    /// Queued jobs in repositories known to have work (refreshed on scrape; at most 1000 per repository).
    pub jobs_queued: IntGauge,
    /// Seconds since the server started.
    pub uptime: IntGauge,
    /// Trace spans delivered to the collector.
    pub trace_exported: IntCounter,
    /// Trace spans dropped (queue full or export failed).
    pub trace_dropped: IntCounter,
    /// Failed trace export attempts.
    pub trace_failures: IntCounter,
    /// Always 1; the labels say which replica this is, which build, and which control database it uses.
    pub replica_info: IntGaugeVec,
    /// Schema version of the control database as found at start-up.
    pub control_schema_version: IntGauge,
    /// 1 from SIGTERM until exit (the replica reports "not ready" and finishes its requests).
    pub draining: IntGauge,
    /// Seconds between an event being recorded and this replica delivering it to a subscriber.
    pub event_lag: Histogram,
    /// Cluster notices by direction (`sent`, `received`) and kind (`events`, `jobs`, `bindings`).
    pub cluster_notices: IntCounterVec,
    /// 1 while this replica's `LISTEN` connection to the control database is up (always 0 on SQLite).
    pub cluster_listener_up: IntGauge,
    /// Times the listener (re)connected and refreshed everything, as after a possible gap in notices.
    pub cluster_resyncs: IntCounter,
}

impl Metrics {
    /// Registers every metric.
    pub fn new() -> Self {
        let registry = Registry::new_custom(Some("agwp".into()), None).expect("valid prefix");
        let requests =
            IntCounterVec::new(Opts::new("requests_total", "Requests handled"), &["transport", "operation", "code"])
                .expect("metric");
        let latency = HistogramVec::new(
            HistogramOpts::new("request_duration_seconds", "Request latency").buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
            ]),
            &["transport", "operation"],
        )
        .expect("metric");
        let inflight = IntGauge::new("inflight_requests", "Requests being handled").expect("metric");
        let auth_failures = IntCounterVec::new(Opts::new("auth_failures_total", "Failed authentications"), &["reason"])
            .expect("metric");
        let rate_limited =
            IntCounter::new("rate_limited_total", "Requests rejected by the rate limiter").expect("metric");
        let shed = IntCounter::new("shed_total", "Requests shed under overload").expect("metric");
        let integrations =
            IntCounterVec::new(Opts::new("integrations_total", "Integration attempts by outcome"), &["outcome"])
                .expect("metric");
        let object_bytes_in = IntCounter::new("object_bytes_received_total", "Object bytes received").expect("metric");
        let object_bytes_out = IntCounter::new("object_bytes_sent_total", "Object bytes sent").expect("metric");
        let open_repos = IntGauge::new("open_repositories", "Repositories currently open").expect("metric");
        let subscriptions = IntGauge::new("event_subscriptions", "Open event subscriptions").expect("metric");
        let job_events = IntCounterVec::new(Opts::new("job_events_total", "Execution-plane job events"), &["event"])
            .expect("metric");
        let jobs_queued = IntGauge::new("jobs_queued", "Queued execution-plane jobs").expect("metric");
        let uptime = IntGauge::new("uptime_seconds", "Seconds since start").expect("metric");
        let trace_exported =
            IntCounter::new("trace_spans_exported_total", "Trace spans delivered to the collector").expect("metric");
        let trace_dropped =
            IntCounter::new("trace_spans_dropped_total", "Trace spans dropped before delivery").expect("metric");
        let trace_failures =
            IntCounter::new("trace_export_failures_total", "Failed trace export attempts").expect("metric");
        let replica_info = IntGaugeVec::new(
            Opts::new("replica_info", "Which replica, build and control database this is (value 1)"),
            &["replica", "version", "control"],
        )
        .expect("metric");
        let control_schema_version =
            IntGauge::new("control_schema_version", "Control database schema version at start-up").expect("metric");
        let draining = IntGauge::new("draining", "1 while the replica is draining before shutdown").expect("metric");
        let event_lag = Histogram::with_opts(
            HistogramOpts::new(
                "event_delivery_lag_seconds",
                "Time from an event being recorded to its delivery to a subscriber",
            )
            .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0]),
        )
        .expect("metric");
        let cluster_notices = IntCounterVec::new(
            Opts::new("cluster_notices_total", "Notices exchanged between replicas through the control database"),
            &["direction", "kind"],
        )
        .expect("metric");
        let cluster_listener_up =
            IntGauge::new("cluster_listener_up", "1 while the cluster notice connection is up").expect("metric");
        let cluster_resyncs = IntCounter::new(
            "cluster_resyncs_total",
            "Full refreshes after the cluster notice connection (re)connected",
        )
        .expect("metric");
        for c in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(latency.clone()),
            Box::new(inflight.clone()),
            Box::new(auth_failures.clone()),
            Box::new(rate_limited.clone()),
            Box::new(shed.clone()),
            Box::new(integrations.clone()),
            Box::new(object_bytes_in.clone()),
            Box::new(object_bytes_out.clone()),
            Box::new(open_repos.clone()),
            Box::new(subscriptions.clone()),
            Box::new(job_events.clone()),
            Box::new(jobs_queued.clone()),
            Box::new(uptime.clone()),
            Box::new(trace_exported.clone()),
            Box::new(trace_dropped.clone()),
            Box::new(trace_failures.clone()),
            Box::new(replica_info.clone()),
            Box::new(control_schema_version.clone()),
            Box::new(draining.clone()),
            Box::new(event_lag.clone()),
            Box::new(cluster_notices.clone()),
            Box::new(cluster_listener_up.clone()),
            Box::new(cluster_resyncs.clone()),
        ] {
            registry.register(c).expect("register metric");
        }
        Self {
            registry,
            requests,
            latency,
            inflight,
            auth_failures,
            rate_limited,
            shed,
            integrations,
            object_bytes_in,
            object_bytes_out,
            open_repos,
            subscriptions,
            job_events,
            jobs_queued,
            uptime,
            trace_exported,
            trace_dropped,
            trace_failures,
            replica_info,
            control_schema_version,
            draining,
            event_lag,
            cluster_notices,
            cluster_listener_up,
            cluster_resyncs,
        }
    }

    /// Renders the Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut buf = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut buf).expect("encode");
        String::from_utf8(buf).unwrap_or_default()
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_render_with_the_agwp_prefix() {
        let m = Metrics::new();
        m.requests.with_label_values(&["http", "repo.describe", "ok"]).inc();
        m.latency.with_label_values(&["http", "repo.describe"]).observe(0.012);
        m.integrations.with_label_values(&["integrated"]).inc();
        let text = m.render();
        assert!(text.contains("agwp_requests_total{code=\"ok\",operation=\"repo.describe\",transport=\"http\"} 1"));
        assert!(text.contains("agwp_request_duration_seconds_bucket"));
        assert!(text.contains("agwp_integrations_total{outcome=\"integrated\"} 1"));
    }
}
