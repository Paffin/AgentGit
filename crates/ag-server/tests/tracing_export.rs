//! Spans reach an OTLP/HTTP collector, continue incoming traces and never include secrets.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::routing::post;
use common::*;
use serde_json::Value;

#[derive(Clone, Default)]
struct Collector {
    bodies: Arc<Mutex<Vec<Value>>>,
    headers: Arc<Mutex<Vec<HeaderMap>>>,
    status: Arc<Mutex<u16>>,
}

impl Collector {
    async fn start(status: u16) -> (Self, String) {
        let c = Collector { status: Arc::new(Mutex::new(status)), ..Default::default() };
        let state = c.clone();
        let app = Router::new().route(
            "/v1/traces",
            post(move |headers: HeaderMap, body: Bytes| {
                let state = state.clone();
                async move {
                    state.bodies.lock().unwrap().push(serde_json::from_slice(&body).unwrap_or(Value::Null));
                    state.headers.lock().unwrap().push(headers);
                    axum::http::StatusCode::from_u16(*state.status.lock().unwrap()).unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (c, url)
    }

    fn spans(&self) -> Vec<Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .flat_map(|b| b["resourceSpans"].as_array().cloned().unwrap_or_default())
            .flat_map(|rs| rs["scopeSpans"].as_array().cloned().unwrap_or_default())
            .flat_map(|ss| ss["spans"].as_array().cloned().unwrap_or_default())
            .collect()
    }
}

fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a str> {
    span["attributes"].as_array()?.iter().find(|a| a["key"] == key)?["value"]["stringValue"].as_str()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operations_become_spans_that_continue_incoming_traces() {
    let (collector, url) = Collector::start(200).await;
    let srv = TestServer::start_with(|cfg| {
        cfg.telemetry.otlp_endpoint = url.clone();
        cfg.telemetry.service_name = "ag-test".into();
    })
    .await;
    let admin = srv.admin();
    let trace = "4bf92f3577b34da6a3ce929d0e0e4736";
    let caller_span = "00f067aa0ba902b7";
    let traced =
        admin.call("GET", "/v1/whoami", None, &[("traceparent", &format!("00-{trace}-{caller_span}-01"))]).await;
    assert_eq!(traced.status, 200);
    // A caller that decided not to sample is respected.
    let skipped_trace = "11111111111111111111111111111111";
    admin.call("GET", "/v1/whoami", None, &[("traceparent", &format!("00-{skipped_trace}-2222222222222222-00"))]).await;
    // A request without traceparent starts its own trace; a missing repository is a caller error, not a fault.
    admin.get("/v1/repos/acme/nope").await;
    // A request with a bad credential still produces a span, without the credential in it.
    srv.client("agt_secretsecretsecretsecret").get("/v1/whoami").await;
    srv.stop().await; // flushes the exporter

    let spans = collector.spans();
    let continued: Vec<&Value> = spans.iter().filter(|s| s["traceId"] == trace).collect();
    assert_eq!(continued.len(), 1, "{spans:#?}");
    let s = continued[0];
    assert_eq!(s["parentSpanId"], caller_span);
    assert_eq!(s["name"], "session.whoami");
    assert_eq!(s["kind"], 2);
    assert_eq!(s["status"]["code"], 1);
    assert_eq!(attr(s, "agwp.transport"), Some("http"));
    assert_eq!(attr(s, "agwp.result"), Some("ok"));
    let (start, end): (u64, u64) = (
        s["startTimeUnixNano"].as_str().unwrap().parse().unwrap(),
        s["endTimeUnixNano"].as_str().unwrap().parse().unwrap(),
    );
    assert!(end >= start && start > 1_600_000_000_000_000_000);
    assert_eq!(s["spanId"].as_str().unwrap().len(), 16);
    assert!(spans.iter().all(|s| s["traceId"] != skipped_trace), "an unsampled parent must not be recorded");

    let not_found = spans.iter().find(|s| attr(s, "agwp.result") == Some("not_found")).expect("the 404 is recorded");
    assert_eq!(not_found["status"]["code"], 1, "client errors are not span errors");
    assert!(not_found.get("parentSpanId").is_none() || not_found["parentSpanId"].is_null());
    assert_eq!(not_found["traceId"].as_str().unwrap().len(), 32);

    // Resource and secrets.
    let body = collector.bodies.lock().unwrap()[0].clone();
    let res = body["resourceSpans"][0]["resource"]["attributes"].to_string();
    assert!(res.contains("ag-test") && res.contains("service.version"), "{res}");
    let everything = serde_json::to_string(&*collector.bodies.lock().unwrap()).unwrap();
    assert!(
        !everything.contains("secretsecret") && !everything.contains(ADMIN_TOKEN),
        "credentials must never be exported"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_collector_never_fails_requests_and_is_counted() {
    let (collector, url) = Collector::start(500).await;
    let srv = TestServer::start_with(|cfg| cfg.telemetry.otlp_endpoint = url.clone()).await;
    let admin = srv.admin();
    for _ in 0..5 {
        assert_eq!(admin.get("/v1/whoami").await.status, 200);
    }
    if let Some(t) = &srv.app.tracer {
        let t = t.clone();
        tokio::task::spawn_blocking(move || t.flush()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    assert!(m.contains("agwp_trace_export_failures_total 1"), "{m}");
    assert!(m.contains("agwp_trace_spans_dropped_total 5"), "{m}");
    assert!(m.contains("agwp_trace_spans_exported_total 0"), "{m}");
    assert_eq!(collector.headers.lock().unwrap()[0]["content-type"], "application/json");
    srv.stop().await;
}

/// The real binary: configuration file, headers from the environment, flush on SIGTERM.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_binary_exports_with_headers_from_the_environment_and_flushes_on_sigterm() {
    let (collector, url) = Collector::start(200).await;
    let dir = tempfile::tempdir().unwrap();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let config = dir.path().join("ag.toml");
    std::fs::write(
        &config,
        format!(
            "[server]\nlisten = \"127.0.0.1:{port}\"\ndata_dir = {:?}\n[telemetry]\notlp_endpoint = {url:?}\nservice_name = \"ag-binary\"\n",
            dir.path().join("data")
        ),
    )
    .unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ag-server"))
        .args(["serve", "--config"])
        .arg(&config)
        .env("AG_BOOTSTRAP_TOKEN", ADMIN_TOKEN)
        .env("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer collector-token, x-tenant=acme")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let http = Http { base: format!("http://127.0.0.1:{port}"), token: ADMIN_TOKEN.into() };
    let mut up = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "the server did not start");
    assert_eq!(http.get("/v1/whoami").await.status, 200);
    // SIGTERM triggers the graceful shutdown, which flushes the exporter.
    let status = std::process::Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(status.success());
    let exit = tokio::task::spawn_blocking(move || child.wait().unwrap()).await.unwrap();
    assert!(exit.success(), "{exit:?}");
    let spans = collector.spans();
    assert!(spans.iter().any(|s| s["name"] == "session.whoami"), "{spans:#?}");
    let h = collector.headers.lock().unwrap();
    assert_eq!(h[0]["authorization"], "Bearer collector-token");
    assert_eq!(h[0]["x-tenant"], "acme");
    let body = collector.bodies.lock().unwrap()[0].to_string();
    assert!(body.contains("ag-binary"));
}

#[tokio::test]
async fn tracing_is_off_by_default_and_configuration_is_validated() {
    let srv = TestServer::start().await;
    assert!(srv.app.tracer.is_none());
    srv.stop().await;
    let mut cfg = ag_server::config::ServerConfig::default();
    for bad in ["collector:4318", "http://", "http://user:pw@collector:4318", "ftp://x"] {
        cfg.telemetry.otlp_endpoint = bad.into();
        assert!(cfg.validate().is_err(), "{bad}");
    }
    cfg.telemetry.otlp_endpoint = "https://collector.example:4318".into();
    cfg.validate().unwrap();
    cfg.telemetry.otlp_sample_ratio = 1.5;
    assert!(cfg.validate().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_healthcheck_subcommand_reports_readiness() {
    let srv = TestServer::start().await;
    let ok = tokio::task::spawn_blocking({
        let base = srv.base.clone();
        move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_ag-server"))
                .args(["healthcheck", "--url", &base])
                .status()
                .unwrap()
        }
    })
    .await
    .unwrap();
    assert!(ok.success());
    srv.stop().await;
    let down = std::process::Command::new(env!("CARGO_BIN_EXE_ag-server"))
        .args(["healthcheck", "--url", "http://127.0.0.1:1", "--timeout", "1"])
        .status()
        .unwrap();
    assert_eq!(down.code(), Some(1));
}
