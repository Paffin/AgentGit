//! Prometheus metrics.

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder,
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
    /// Execution-plane jobs by state (refreshed on scrape).
    pub jobs: IntGaugeVec,
    /// Seconds since the server started.
    pub uptime: IntGauge,
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
        let jobs = IntGaugeVec::new(Opts::new("jobs", "Execution-plane jobs by state"), &["state"]).expect("metric");
        let uptime = IntGauge::new("uptime_seconds", "Seconds since start").expect("metric");
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
            Box::new(jobs.clone()),
            Box::new(uptime.clone()),
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
            jobs,
            uptime,
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
