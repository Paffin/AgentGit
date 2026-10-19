//! Events over SSE, metrics, and the OpenAPI document.

mod common;

use std::io::{BufRead, BufReader};
use std::time::Duration;

use common::*;
use serde_json::json;

/// Reads SSE frames from `path` on a blocking thread, sending each parsed
/// `(id, event, data)` over a channel until `stop` frames arrived.
fn sse(
    base: String,
    token: String,
    path: String,
    headers: Vec<(String, String)>,
    tx: std::sync::mpsc::Sender<(String, String, String)>,
) {
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        let mut req = agent.get(format!("{base}{path}")).header("authorization", format!("Bearer {token}"));
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let Ok(mut resp) = req.call() else { return };
        let reader = BufReader::new(resp.body_mut().as_reader());
        let (mut id, mut event, mut data) = (String::new(), String::new(), String::new());
        for line in reader.lines() {
            let Ok(line) = line else { return };
            if line.is_empty() {
                if !data.is_empty() && tx.send((id.clone(), event.clone(), data.clone())).is_err() {
                    return;
                }
                id.clear();
                event.clear();
                data.clear();
            } else if let Some(v) = line.strip_prefix("id:") {
                id = v.trim().into();
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().into();
            } else if let Some(v) = line.strip_prefix("data:") {
                data = v.trim().into();
            }
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_sent_events_replay_then_follow_and_resume() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;

    // Follow from "now".
    let (tx, rx) = std::sync::mpsc::channel();
    sse(srv.base.clone(), alice.token.clone(), "/v1/repos/acme/web/events".into(), vec![], tx);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let c = Client::new("acme/web");
    let (head, objs) = c.commit(vec![], "user:alice", "first", &[("a.txt", b"a")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objs).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": head.to_string(), "title": "first"}))
        .await;
    let pid = p.json["id"].as_str().unwrap().to_string();
    carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;

    let mut seen = Vec::new();
    let mut last_id = 0u64;
    while let Ok((id, event, data)) = rx.recv_timeout(Duration::from_secs(10)) {
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["topic"], event.as_str());
        assert_eq!(v["repo"], "acme/web");
        last_id = id.parse().unwrap();
        seen.push(event.clone());
        if event == "stream.integrate" {
            assert_eq!(v["payload"]["details"]["new_head"], head.to_string());
            break;
        }
    }
    assert!(seen.contains(&"proposal.create".to_string()), "{seen:?}");

    // Resume after the last seen id (Last-Event-ID): only newer events arrive.
    let pin = admin.post("/v1/repos/acme/web/pins", json!({"name": "keep", "rev": head.to_string()})).await;
    assert_eq!(pin.status, 200, "{}", pin.json);
    let (tx2, rx2) = std::sync::mpsc::channel();
    sse(
        srv.base.clone(),
        alice.token.clone(),
        "/v1/repos/acme/web/events".into(),
        vec![("last-event-id".into(), last_id.to_string())],
        tx2,
    );
    let replay = alice.get(&format!("/v1/repos/acme/web/events/replay?after_seq={last_id}")).await;
    let got = rx2.recv_timeout(Duration::from_secs(10));
    let (id, event, _) = got
        .unwrap_or_else(|_| panic!("the missed event is not streamed; replay says {}, last_id={last_id}", replay.json));
    assert!(id.parse::<u64>().unwrap() > last_id);
    assert_eq!(event, "pin.set");

    // Topic filters and access control.
    let (tx3, rx3) = std::sync::mpsc::channel();
    sse(
        srv.base.clone(),
        alice.token.clone(),
        "/v1/repos/acme/web/events?after_seq=0&topics=stream".into(),
        vec![],
        tx3,
    );
    let (_, event, _) = rx3.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(event.starts_with("stream."), "only stream.* topics: {event}");
    let outsider = user_with_role(&srv, "user:eve", "other/*", "reader").await;
    assert_eq!(outsider.get("/v1/repos/acme/web/events/replay").await.status, 404);
    srv.stop().await;
}

/// Appends `n` audit entries of a topic nobody asked for, the way a busy repository does (job claims, lease
/// renewals, ...), straight into the repository's own audit log.
fn noise(srv: &TestServer, repo: &str, n: usize) {
    let handle = srv.app.repos.handle(repo).unwrap();
    handle
        .meta()
        .write(|tx| {
            for i in 0..n {
                tx.audit_append("system:test", "lease.renew", "lease_x", &json!({ "i": i }))?;
            }
            Ok(())
        })
        .unwrap();
}

fn latest_seq(srv: &TestServer, repo: &str) -> u64 {
    srv.app.repos.handle(repo).unwrap().meta().read(|tx| tx.audit_latest_seq()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topic_filtered_feed_never_skips_an_event_behind_a_run_of_other_entries() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let start = latest_seq(&srv, "acme/web");
    // 2600 entries nobody asked for, then the one event the subscriber wants. The old reader looked at 4 x limit
    // rows (1024 for a live feed, 2000 for a default replay), found nothing, and moved the cursor to the end of
    // the log: the event behind the run was lost for good.
    noise(&srv, "acme/web", 2600);
    let made = admin.post("/v1/repos/acme/web/streams", json!({"name": "dev"})).await;
    assert_eq!(made.status, 200, "{}", made.json);
    noise(&srv, "acme/web", 30);

    // Replay: the matching event is returned, and a page that is not full says the log is exhausted.
    let r = admin.get(&format!("/v1/repos/acme/web/events/replay?topics=stream&after_seq={start}")).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let topics: Vec<&str> = r.json["events"].as_array().unwrap().iter().map(|e| e["topic"].as_str().unwrap()).collect();
    assert_eq!(topics, ["stream.create"], "{}", r.json);
    assert_eq!(r.json["latest_seq"], latest_seq(&srv, "acme/web"));
    // With a small limit the page is full and the next one continues from its last event.
    admin.post("/v1/repos/acme/web/streams", json!({"name": "dev2"})).await;
    let first = admin.get(&format!("/v1/repos/acme/web/events/replay?topics=stream&after_seq={start}&limit=1")).await;
    let events = first.json["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{}", first.json);
    let next = admin
        .get(&format!(
            "/v1/repos/acme/web/events/replay?topics=stream&after_seq={}&limit=1",
            events[0]["seq"].as_u64().unwrap()
        ))
        .await;
    assert_eq!(next.json["events"][0]["payload"]["target"], "dev2", "{}", next.json);

    // Live: a subscriber that resumes from before the run receives the event behind it.
    let (tx, rx) = std::sync::mpsc::channel();
    sse(
        srv.base.clone(),
        admin.token.clone(),
        format!("/v1/repos/acme/web/events?topics=stream&after_seq={start}"),
        vec![],
        tx,
    );
    let (_, event, _) = rx.recv_timeout(Duration::from_secs(10)).expect("the event behind the run is delivered");
    assert_eq!(event, "stream.create");
    // ... and keeps following: noise is skipped quietly, the next matching event arrives.
    noise(&srv, "acme/web", 1500);
    admin.post("/v1/repos/acme/web/streams", json!({"name": "dev3"})).await;
    let mut targets = Vec::new();
    while let Ok((_, event, data)) = rx.recv_timeout(Duration::from_secs(10)) {
        assert_eq!(event, "stream.create");
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        targets.push(v["payload"]["target"].as_str().unwrap().to_string());
        if targets.contains(&"dev3".to_string()) {
            break;
        }
    }
    assert_eq!(targets, ["dev2", "dev3"], "nothing lost, nothing repeated");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_event_stream_ends_when_its_credential_is_revoked() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let minted = alice.post("/v1/credentials", json!({"ttl_secs": 600})).await;
    assert_eq!(minted.status, 200, "{}", minted.json);
    let (id, token) = (
        minted.json["credential"]["id"].as_str().unwrap().to_string(),
        minted.json["token"].as_str().unwrap().to_string(),
    );

    let (tx, rx) = std::sync::mpsc::channel();
    sse(srv.base.clone(), token, "/v1/repos/acme/web/events".into(), vec![], tx);
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The stream works while the credential does ...
    alice.post("/v1/repos/acme/web/streams", json!({"name": "one"})).await;
    let (_, event, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(event, "stream.create");
    // ... and not a moment after it is revoked: the very next batch is withheld and the stream ends with an error,
    // without waiting for a timer. (The old code looked at the credential every 5 s and delivered whatever came
    // in between.)
    let revoked = alice.call("DELETE", &format!("/v1/credentials/{id}"), None, &[]).await;
    assert_eq!(revoked.status, 200, "{}", revoked.json);
    admin.post("/v1/repos/acme/web/streams", json!({"name": "two"})).await;
    let (_, event, data) = rx.recv_timeout(Duration::from_secs(10)).expect("the stream ends with an error event");
    assert_eq!(event, "error", "no event may be delivered after the revocation: {data}");
    assert!(data.contains("unauthenticated"), "{data}");
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_err(), "and then nothing more");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_event_queries_are_rejected_not_replaced_by_defaults() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    for path in [
        "/v1/repos/acme/web/events/replay?after_seq=abc",
        "/v1/repos/acme/web/events/replay?after_seq=-5",
        "/v1/repos/acme/web/events/replay?limit=many",
        "/v1/repos/acme/web/events?after_seq=undefined",
        "/v1/repos/acme/web/events?heartbeat_ms=soon",
    ] {
        let r = admin.get(path).await;
        assert_eq!(r.status, 400, "{path}: {}", r.json);
        assert_eq!(r.json["error"]["code"], "invalid_argument", "{path}");
    }
    // A resume position that is not a sequence number must not turn into "follow from now".
    let r = admin.call("GET", "/v1/repos/acme/web/events", None, &[("last-event-id", "not-a-number")]).await;
    assert_eq!(r.status, 400, "{}", r.json);
    // Well-formed queries (and absent ones) still work.
    assert_eq!(admin.get("/v1/repos/acme/web/events/replay?after_seq=0&limit=5&topics=repo").await.status, 200);
    assert_eq!(admin.get("/v1/repos/acme/web/events/replay").await.status, 200);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_count_requests_by_operation_and_outcome() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    admin.get("/v1/repos/acme/web").await;
    srv.client("bad-token-xxxxxxxxxxxxxxxx").get("/v1/whoami").await;
    admin.get("/v1/repos/acme/nope").await;

    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    assert!(m.contains(r#"agwp_requests_total{code="ok",operation="repo.create",transport="http"} 1"#), "{m}");
    assert!(m.contains(r#"operation="repo.describe""#));
    assert!(m.contains(r#"code="not_found""#));
    assert!(m.contains(r#"agwp_auth_failures_total{reason="invalid"} 1"#));
    assert!(m.contains("agwp_request_duration_seconds_bucket"));
    assert!(m.contains("agwp_open_repositories 1"));
    assert!(m.contains("agwp_uptime_seconds"));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_openapi_document_matches_the_router_and_the_checked_in_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ag_server::config::ServerConfig::default();
    cfg.server.data_dir = dir.path().to_path_buf();
    let app = ag_server::app::App::new(cfg).unwrap();
    let (_, routes) = ag_server::server::build_router(app);
    let spec = ag_server::routes::openapi(&routes, "0.1.0");

    // Every route is documented exactly once.
    let mut seen = std::collections::BTreeSet::new();
    for r in &routes {
        assert!(seen.insert((r.method, r.path)), "duplicate route {} {}", r.method, r.path);
        assert!(spec["paths"][r.path][r.method]["operationId"] == r.operation, "{} {}", r.method, r.path);
    }
    assert!(routes.len() > 80, "the table lists the whole protocol, got {}", routes.len());
    // Operation ids are unique.
    let ops: std::collections::BTreeSet<_> = routes.iter().map(|r| r.operation).collect();
    assert_eq!(ops.len(), routes.len());
    // Message schemas are generated from the wire types.
    for name in ["ErrorInfo", "Proposal", "IntegrationResult", "Preparation", "Credential", "Event", "ErrorEnvelope"] {
        assert!(spec["components"]["schemas"][name].is_object(), "schema {name} missing");
    }
    assert_eq!(spec["paths"]["/v1/capabilities"]["get"]["security"], json!([]), "capabilities are public");

    // The checked-in copy (docs/protocol/openapi.json) is what SDKs are generated from;
    // regenerate it with AG_UPDATE_GOLDEN=1 cargo test -p agentgit-server --test observability.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/protocol/openapi.json");
    let fresh = serde_json::to_string_pretty(&spec).unwrap() + "\n";
    if std::env::var_os("AG_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &fresh).unwrap();
    }
    let stored = std::fs::read_to_string(path).expect("docs/protocol/openapi.json (run with AG_UPDATE_GOLDEN=1)");
    assert!(
        stored == fresh,
        "docs/protocol/openapi.json is out of date; run AG_UPDATE_GOLDEN=1 cargo test -p agentgit-server --test observability"
    );
}
