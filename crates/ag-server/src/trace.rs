//! Distributed tracing: one span per handled operation, exported as OTLP/HTTP JSON.
//!
//! This is deliberately small. Every operation that passes through
//! [`crate::app::App::dispatch`] becomes a `SERVER` span named after the
//! operation (`proposals.create`, `integration.integrate`, ...). An incoming W3C
//! `traceparent` is continued (same trace id, the caller's span as parent);
//! without one a new trace starts. A parent that was **not sampled** is
//! respected: nothing is recorded for it.
//!
//! Export never slows or fails a request: spans go through a bounded queue to
//! one background thread that batches them and POSTs `…/v1/traces` with
//! `Content-Type: application/json`. When the queue is full, or the collector
//! is down, spans are dropped and counted (`agwp_trace_spans_dropped_total`,
//! `agwp_trace_export_failures_total`). On shutdown the queue is flushed.
//!
//! Spans carry the operation, transport, result code and request id — never
//! credentials, request bodies or identities.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prometheus::IntCounter;
use serde_json::{Value, json};

const QUEUE: usize = 4096;
const BATCH: usize = 256;
const FLUSH_EVERY: Duration = Duration::from_secs(2);
const POST_TIMEOUT: Duration = Duration::from_secs(5);

/// Counters the exporter updates (registered in [`crate::metrics::Metrics`]).
#[derive(Clone)]
pub struct TraceCounters {
    /// Spans delivered to the collector.
    pub exported: IntCounter,
    /// Spans discarded (queue full, or the collector rejected the batch).
    pub dropped: IntCounter,
    /// Failed export attempts (batches).
    pub failures: IntCounter,
}

/// A parsed W3C `traceparent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceParent {
    /// 16-byte trace id (never all zero).
    pub trace_id: [u8; 16],
    /// 8-byte id of the caller's span (never all zero).
    pub span_id: [u8; 8],
    /// The `sampled` flag.
    pub sampled: bool,
}

impl TraceParent {
    /// Parses `00-<32 hex>-<16 hex>-<2 hex>`; anything malformed is ignored (None).
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split('-');
        let (version, trace, span, flags) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        // Versions above 00 may append fields; version ff is invalid.
        if version.len() != 2 || version == "ff" || trace.len() != 32 || span.len() != 16 || flags.len() != 2 {
            return None;
        }
        if version == "00" && parts.next().is_some() {
            return None;
        }
        let lower_hex = |s: &str| s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !(lower_hex(version) && lower_hex(trace) && lower_hex(span) && lower_hex(flags)) {
            return None;
        }
        let trace_id: [u8; 16] = hex::decode(trace).ok()?.try_into().ok()?;
        let span_id: [u8; 8] = hex::decode(span).ok()?.try_into().ok()?;
        if trace_id == [0; 16] || span_id == [0; 8] {
            return None;
        }
        let flags = u8::from_str_radix(flags, 16).ok()?;
        Some(Self { trace_id, span_id, sampled: flags & 1 == 1 })
    }
}

/// Attribute value.
#[derive(Clone, Debug)]
pub enum Attr {
    /// Text.
    Str(String),
    /// Integer.
    Int(i64),
}

/// A finished span, ready to export.
#[derive(Clone, Debug)]
pub struct SpanRecord {
    /// Trace id.
    pub trace_id: [u8; 16],
    /// This span's id.
    pub span_id: [u8; 8],
    /// The caller's span, if the trace was continued.
    pub parent: Option<[u8; 8]>,
    /// Operation name.
    pub name: String,
    /// Start, Unix nanoseconds.
    pub start_ns: u64,
    /// End, Unix nanoseconds.
    pub end_ns: u64,
    /// Attributes.
    pub attrs: Vec<(&'static str, Attr)>,
    /// Set when the operation failed: the error code.
    pub error: Option<String>,
}

impl SpanRecord {
    fn to_json(&self) -> Value {
        let attrs: Vec<Value> = self
            .attrs
            .iter()
            .map(|(k, v)| match v {
                Attr::Str(s) => json!({"key": k, "value": {"stringValue": s}}),
                // OTLP/JSON carries 64-bit integers as strings.
                Attr::Int(i) => json!({"key": k, "value": {"intValue": i.to_string()}}),
            })
            .collect();
        let mut span = json!({
            "traceId": hex::encode(self.trace_id),
            "spanId": hex::encode(self.span_id),
            "name": self.name,
            "kind": 2, // SPAN_KIND_SERVER
            "startTimeUnixNano": self.start_ns.to_string(),
            "endTimeUnixNano": self.end_ns.to_string(),
            "attributes": attrs,
            "status": match &self.error {
                Some(code) => json!({"code": 2, "message": code}),
                None => json!({"code": 1}),
            },
        });
        if let Some(p) = self.parent {
            span["parentSpanId"] = json!(hex::encode(p));
        }
        span
    }
}

enum Msg {
    Span(Box<SpanRecord>),
    Flush(SyncSender<()>),
}

/// The exporter handle held by the application.
pub struct Tracer {
    tx: SyncSender<Msg>,
    ratio: f64,
    counters: TraceCounters,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Where and how to export.
#[derive(Clone, Debug)]
pub struct TracerConfig {
    /// Base URL of the OTLP/HTTP collector (`http://host:4318`).
    pub endpoint: String,
    /// `service.name`.
    pub service_name: String,
    /// `service.version`.
    pub service_version: String,
    /// Fraction of *new* traces recorded (0..=1). Continued traces follow their parent's flag.
    pub sample_ratio: f64,
    /// Extra request headers (e.g. `authorization` for a hosted collector).
    pub headers: Vec<(String, String)>,
}

impl Tracer {
    /// Starts the exporter thread.
    pub fn start(cfg: TracerConfig, counters: TraceCounters) -> Arc<Self> {
        let (tx, rx) = sync_channel(QUEUE);
        let worker_counters = counters.clone();
        let ratio = cfg.sample_ratio.clamp(0.0, 1.0);
        let handle = std::thread::Builder::new()
            .name("ag-otlp".into())
            .spawn(move || export_loop(cfg, rx, worker_counters))
            .expect("spawn exporter thread");
        Arc::new(Self { tx, ratio, counters, worker: Mutex::new(Some(handle)) })
    }

    /// Decides whether to record an operation and, if so, returns the ids it will use.
    pub fn begin(&self, traceparent: Option<&str>) -> Option<SpanContext> {
        let span_id = nonzero::<8>();
        match traceparent.and_then(TraceParent::parse) {
            Some(p) if !p.sampled => None,
            Some(p) => Some(SpanContext { trace_id: p.trace_id, span_id, parent: Some(p.span_id), started: now_ns() }),
            None => {
                let trace_id = nonzero::<16>();
                // Deterministic in the trace id, so every hop of one trace agrees.
                let bucket = u64::from_be_bytes(trace_id[8..16].try_into().expect("8 bytes")) as f64 / u64::MAX as f64;
                (bucket < self.ratio || self.ratio >= 1.0).then_some(SpanContext {
                    trace_id,
                    span_id,
                    parent: None,
                    started: now_ns(),
                })
            }
        }
    }

    /// Queues a finished span. Never blocks.
    pub fn finish(&self, ctx: SpanContext, name: &str, attrs: Vec<(&'static str, Attr)>, error: Option<String>) {
        let rec = SpanRecord {
            trace_id: ctx.trace_id,
            span_id: ctx.span_id,
            parent: ctx.parent,
            name: name.to_string(),
            start_ns: ctx.started,
            end_ns: now_ns().max(ctx.started),
            attrs,
            error,
        };
        match self.tx.try_send(Msg::Span(Box::new(rec))) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => self.counters.dropped.inc(),
        }
    }

    /// Sends everything queued so far and waits (bounded) for the attempt to finish.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = sync_channel(1);
        if self.tx.send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(POST_TIMEOUT + Duration::from_secs(2));
        }
    }

    /// Flushes and stops the exporter thread. Call from a blocking thread.
    pub fn shutdown(&self) {
        self.flush();
        let handle = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        // Dropping the last sender ends the loop; the sender lives in `self`, so detach instead:
        // the thread exits when the process does, after the flush above has delivered everything.
        drop(handle);
    }
}

/// Ids of a span in progress.
#[derive(Clone, Copy, Debug)]
pub struct SpanContext {
    /// Trace id.
    pub trace_id: [u8; 16],
    /// This span's id.
    pub span_id: [u8; 8],
    /// The caller's span.
    pub parent: Option<[u8; 8]>,
    started: u64,
}

/// The trace id (32 hex digits) a W3C `traceparent` header names, whether or not the caller sampled the trace:
/// what a request's log lines carry when the exporter records nothing for it.
pub fn trace_id_of(traceparent: &str) -> Option<String> {
    TraceParent::parse(traceparent).map(|p| hex::encode(p.trace_id))
}

fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

fn nonzero<const N: usize>() -> [u8; N] {
    loop {
        if let Ok(b) = ag_crypto::random_bytes::<N>()
            && b != [0; N]
        {
            return b;
        }
    }
}

fn export_loop(cfg: TracerConfig, rx: Receiver<Msg>, counters: TraceCounters) {
    let url = format!("{}/v1/traces", cfg.endpoint.trim_end_matches('/'));
    let agent: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(POST_TIMEOUT)).build().into();
    let resource = json!({"attributes": [
        {"key": "service.name", "value": {"stringValue": cfg.service_name}},
        {"key": "service.version", "value": {"stringValue": cfg.service_version}},
        {"key": "telemetry.sdk.name", "value": {"stringValue": "agentgit"}},
    ]});
    let mut batch: Vec<SpanRecord> = Vec::new();
    let mut deadline = Instant::now() + FLUSH_EVERY;
    let mut last_warning: Option<Instant> = None;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let mut ack = None;
        let mut closed = false;
        match rx.recv_timeout(wait) {
            Ok(Msg::Span(s)) => batch.push(*s),
            Ok(Msg::Flush(a)) => ack = Some(a),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => closed = true,
        }
        // Pick up whatever else is already queued, up to a batch.
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(Msg::Span(s)) => batch.push(*s),
                Ok(Msg::Flush(a)) => {
                    ack = Some(a);
                    break;
                }
                Err(_) => break,
            }
        }
        if !batch.is_empty() && (batch.len() >= BATCH || ack.is_some() || closed || Instant::now() >= deadline) {
            let spans: Vec<Value> = batch.iter().map(SpanRecord::to_json).collect();
            let body = json!({"resourceSpans": [{
                "resource": resource,
                "scopeSpans": [{"scope": {"name": "ag-server"}, "spans": spans}],
            }]});
            let mut req = agent.post(&url).header("content-type", "application/json");
            for (k, v) in &cfg.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let n = batch.len() as u64;
            match req.send(body.to_string()) {
                Ok(resp) if resp.status().is_success() => counters.exported.inc_by(n),
                outcome => {
                    counters.failures.inc();
                    counters.dropped.inc_by(n);
                    if last_warning.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                        last_warning = Some(Instant::now());
                        let why = match outcome {
                            Ok(resp) => format!("the collector answered {}", resp.status()),
                            Err(e) => e.to_string(),
                        };
                        tracing::warn!(endpoint = %cfg.endpoint, %why, spans = n, "trace export failed; spans dropped");
                    }
                }
            }
            batch.clear();
        }
        if Instant::now() >= deadline {
            deadline = Instant::now() + FLUSH_EVERY;
        }
        if let Some(a) = ack {
            let _ = a.send(());
        }
        if closed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_parsing_is_strict() {
        let ok = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
        assert!(ok.sampled);
        assert_eq!(hex::encode(ok.span_id), "00f067aa0ba902b7");
        assert!(!TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00").unwrap().sampled);
        for bad in [
            "",
            "garbage",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01", // zero trace id
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01", // zero span id
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01", // forbidden version
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01", // upper case
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "00-4bf92f3577b34da6a3ce929d0e0e47-00f067aa0ba902b7-01", // short
        ] {
            assert!(TraceParent::parse(bad).is_none(), "{bad}");
        }
        // Future versions may append fields.
        assert!(TraceParent::parse("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-more").is_some());
    }

    #[test]
    fn spans_use_the_otlp_json_shape() {
        let rec = SpanRecord {
            trace_id: [1; 16],
            span_id: [2; 8],
            parent: Some([3; 8]),
            name: "proposals.create".into(),
            start_ns: 10,
            end_ns: 20,
            attrs: vec![("agwp.operation", Attr::Str("proposals.create".into())), ("agwp.deadline_ms", Attr::Int(500))],
            error: Some("not_found".into()),
        };
        let v = rec.to_json();
        assert_eq!(v["traceId"], "01".repeat(16));
        assert_eq!(v["parentSpanId"], "03".repeat(8));
        assert_eq!(v["startTimeUnixNano"], "10");
        assert_eq!(v["attributes"][1]["value"]["intValue"], "500");
        assert_eq!(v["status"]["code"], 2);
        assert_eq!(v["kind"], 2);
    }
}
