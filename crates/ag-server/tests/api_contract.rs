//! Smaller contract points: error envelopes everywhere, repeated query parameters, sizes in listings,
//! idempotent creation, rate limits on every operation, and what the OpenAPI document promises.

mod common;

use base64::Engine as _;
use common::*;
use serde_json::{Value, json};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stray_errors_use_the_standard_envelope_with_the_callers_request_id() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;

    let r = admin.call("GET", "/v1/nope", None, &[("x-request-id", "trace-me-1")]).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json["error"]["code"], "not_found");
    assert_eq!(r.json["error"]["request_id"], "trace-me-1");
    assert!(r.json["error"]["message"].as_str().unwrap().contains("/v1/nope"));

    // A method the route does not have: 405, an envelope, the Allow header kept, the id echoed.
    let r = admin.call("DELETE", "/v1/repos/acme/web", None, &[("x-request-id", "trace-me-2")]).await;
    assert_eq!(r.status, 405, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "invalid_argument");
    assert_eq!(r.json["error"]["request_id"], "trace-me-2");
    assert!(r.json["error"]["message"].as_str().unwrap().contains("not allowed"), "{}", r.json);
    assert!(r.headers.get("allow").is_some_and(|a| a.contains("GET")), "{:?}", r.headers);

    // Without a request id the server makes one up.
    let r = admin.call("GET", "/v1/nope", None, &[]).await;
    assert!(r.json["error"]["request_id"].as_str().unwrap().starts_with("req_"));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_query_parameters_fill_list_fields() {
    let srv = TestServer::start().await;
    let anon = srv.client("");
    let one = anon.get("/v1/capabilities?supported_versions=agwp.v1").await;
    assert_eq!(one.status, 200, "{}", one.json);
    let many =
        anon.get("/v1/capabilities?supported_versions=agwp.v1&supported_versions=agwp.v9&features=a&features=b").await;
    assert_eq!(many.status, 200, "{}", many.json);
    // A client that speaks only a future version is still refused, now by the negotiation and not the parser.
    let future = anon.get("/v1/capabilities?supported_versions=agwp.v9").await;
    assert_eq!(future.status, 412, "{}", future.json);
    assert_eq!(future.json["error"]["code"], "failed_precondition");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listings_report_sizes_and_the_documented_file_type_names() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    // Writing the name that reads used to return ("file") is accepted as an alias of "regular".
    let ws = alice.post("/v1/repos/acme/web/workspaces", json!({"name": "w", "stream": "main"})).await;
    assert_eq!(ws.status, 200, "{}", ws.json);
    let ch = alice
        .post(
            "/v1/repos/acme/web/workspaces/w/changes",
            json!({"changes": [
                {"path": "docs/a.txt", "op": "put", "content": b64("12345"), "file_type": "file"},
                {"path": "b.txt", "op": "put", "content": b64("x"), "file_type": "regular"},
                {"path": "link", "op": "put", "content": b64("b.txt"), "file_type": "symlink"},
            ]}),
        )
        .await;
    assert_eq!(ch.status, 200, "{}", ch.json);
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": ch.json["head"], "title": "files"}))
        .await;
    let id = p.json["id"].as_str().unwrap();
    assert_eq!(
        carol.post(&format!("/v1/repos/acme/web/proposals/{id}/integrate"), json!({})).await.json["outcome"]["status"],
        "integrated"
    );

    // The whole-tree listing: real sizes, the documented type names.
    let tree = alice.get("/v1/repos/acme/web/tree?rev=main").await;
    let entries = tree.json["entries"].as_array().unwrap();
    let by_path =
        |p: &str| entries.iter().find(|e| e["path"] == p).unwrap_or_else(|| panic!("{p} missing: {entries:?}"));
    assert_eq!(by_path("docs/a.txt")["size"], 5);
    assert_eq!(by_path("b.txt")["size"], 1);
    assert_eq!(by_path("docs/a.txt")["file_type"], "regular");
    assert_eq!(by_path("link")["file_type"], "symlink");
    // Directory listings and single-path resolution agree.
    let dir = alice.get("/v1/repos/acme/web/tree/list?rev=main&path=docs").await;
    assert_eq!(dir.json["entries"][0]["size"], 5, "{}", dir.json);
    let one = alice.get("/v1/repos/acme/web/tree/resolve?rev=main&path=docs/a.txt").await;
    assert_eq!(one.json["entry"]["size"], 5);
    // Changesets use the same names.
    let cs = alice.get("/v1/repos/acme/web/changeset?rev=main&include_ops=true").await;
    let ops = cs.json["ops"].as_array().cloned().unwrap_or_default();
    assert!(ops.iter().all(|o| matches!(o["file_type"].as_str(), Some("regular" | "symlink") | None)), "{ops:?}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replaying_after_the_end_returns_nothing() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let all = admin.get("/v1/repos/acme/web/events/replay?after_seq=0").await;
    assert!(!all.json["events"].as_array().unwrap().is_empty());
    for beyond in ["18446744073709551615", "9223372036854775807", "9223372036854775808"] {
        let r = admin.get(&format!("/v1/repos/acme/web/events/replay?after_seq={beyond}")).await;
        assert_eq!(r.status, 200, "{}", r.json);
        assert!(r.json["events"].as_array().unwrap().is_empty(), "after_seq={beyond}: {}", r.json);
        assert_eq!(r.json["latest_seq"], all.json["latest_seq"]);
    }
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creations_with_an_idempotency_key_happen_once() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let bot = user_with_role(&srv, "agent:bot", "acme/web", "writer").await;
    let key = [("idempotency-key", "k-1")];

    // Tasks.
    let body = json!({"title": "extract module", "scope": ["src/**"]});
    let a = bot.call("POST", "/v1/repos/acme/web/tasks", Some(body.clone()), &key).await;
    let b = bot.call("POST", "/v1/repos/acme/web/tasks", Some(body.clone()), &key).await;
    assert_eq!((a.status, b.status), (200, 200), "{} {}", a.json, b.json);
    assert_eq!(a.json["id"], b.json["id"], "the repeat returns the recorded task");
    let other = bot.call("POST", "/v1/repos/acme/web/tasks", Some(json!({"title": "something else"})), &key).await;
    assert_eq!(other.status, 422, "{}", other.json);
    assert_eq!(other.json["error"]["code"], "idempotency_key_reused");
    let plain1 = bot.post("/v1/repos/acme/web/tasks", body.clone()).await;
    let plain2 = bot.post("/v1/repos/acme/web/tasks", body.clone()).await;
    assert_ne!(plain1.json["id"], plain2.json["id"], "without a key every call creates");
    assert_eq!(bot.get("/v1/repos/acme/web/tasks").await.json["tasks"].as_array().unwrap().len(), 3);

    // Agent runs.
    let key2 = [("idempotency-key", "run-1")];
    bot.post(
        "/v1/repos/acme/web/agents",
        json!({"name": "bot", "public_key": ag_crypto::KeyPair::generate().unwrap().public().to_hex()}),
    )
    .await;
    let r1 = bot.call("POST", "/v1/repos/acme/web/runs", Some(json!({"agent": "bot"})), &key2).await;
    let r2 = bot.call("POST", "/v1/repos/acme/web/runs", Some(json!({"agent": "bot"})), &key2).await;
    assert_eq!(r1.status, 200, "{}", r1.json);
    assert_eq!(r1.json["run_id"], r2.json["run_id"]);
    assert_eq!(bot.get("/v1/repos/acme/web/runs").await.json["runs"].as_array().unwrap().len(), 1);

    // Leases: an exclusive lease taken twice with the same key is one lease, not a conflict with itself.
    let key3 = [("idempotency-key", "lease-1")];
    let l = json!({"scope": "src/**", "exclusive": true, "ttl_ms": 60000});
    let l1 = bot.call("POST", "/v1/repos/acme/web/leases", Some(l.clone()), &key3).await;
    let l2 = bot.call("POST", "/v1/repos/acme/web/leases", Some(l.clone()), &key3).await;
    assert_eq!(l1.status, 200, "{}", l1.json);
    assert_eq!(l2.status, 200, "{}", l2.json);
    assert_eq!(l1.json["lease"]["id"], l2.json["lease"]["id"]);
    assert_eq!(l1.json["lease"]["fencing_token"], l2.json["lease"]["fencing_token"]);
    // Without a key, repeating the request takes a second lease (and a higher fencing token).
    let l3 = bot.post("/v1/repos/acme/web/leases", l).await;
    assert_eq!(l3.status, 200, "{}", l3.json);
    assert_ne!(l3.json["lease"]["id"], l1.json["lease"]["id"]);
    assert!(l3.json["lease"]["fencing_token"].as_u64() > l1.json["lease"]["fencing_token"].as_u64());
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rate_limit_covers_operations_without_a_repository_too() {
    let srv = TestServer::start_with(|cfg| {
        cfg.limits.requests_per_second = 1;
        cfg.limits.burst = 3;
    })
    .await;
    let admin = srv.admin();
    let mut limited = None;
    for _ in 0..8 {
        let r = admin.get("/v1/whoami").await;
        if r.status == 429 {
            limited = Some(r);
            break;
        }
    }
    let r = limited.expect("a burst of whoami calls must hit the limit");
    assert_eq!(r.json["error"]["code"], "rate_limited");
    assert!(r.json["error"]["retry_after_ms"].as_i64().unwrap() > 0);
    // Health and metrics are not API operations and stay reachable.
    assert_eq!(srv.client("").get("/healthz").await.status, 200);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_openapi_document_describes_the_raw_routes() {
    let srv = TestServer::start().await;
    let spec: Value = srv.client("").get("/openapi.json").await.json;
    let op = |path: &str, method: &str| spec["paths"][path][method].clone();

    let file = op("/v1/repos/{owner}/{rname}/files/{path}", "get");
    let file = if file.is_null() { op("/v1/repos/{owner}/{rname}/files/{*path}", "get") } else { file };
    assert!(file["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "rev" && p["in"] == "query"), "{file}");

    let put = op("/v1/repos/{owner}/{rname}/objects/{object_id}", "put");
    assert!(put["requestBody"]["content"]["application/octet-stream"].is_object(), "{put}");

    let sse = op("/v1/repos/{owner}/{rname}/events", "get");
    let names: Vec<&str> = sse["parameters"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    for n in ["after_seq", "topics", "heartbeat_ms"] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    assert!(sse["responses"]["200"]["content"]["text/event-stream"].is_object(), "{sse}");
    srv.stop().await;
}
