//! Blame over the API, on a history built entirely through the API.

mod common;

use ag_protocol::v1;
use ag_protocol::v1::history_service_client::HistoryServiceClient;
use base64::Engine as _;
use common::*;
use serde_json::{Value, json};
use tonic::Request;
use tonic::metadata::MetadataValue;

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

/// Lands `content` as `path` through a virtual workspace and an integration; returns the new head.
async fn land(writer: &Http, integrator: &Http, name: &str, path: &str, content: &str, message: &str) -> String {
    let ws = writer.post("/v1/repos/acme/web/workspaces", json!({"name": name, "stream": "main"})).await;
    assert_eq!(ws.status, 200, "{}", ws.json);
    let ch = writer
        .post(
            &format!("/v1/repos/acme/web/workspaces/{name}/changes"),
            json!({"message": message, "changes": [{"path": path, "op": "put", "content": b64(content)}]}),
        )
        .await;
    assert_eq!(ch.status, 200, "{}", ch.json);
    let head = ch.json["head"].as_str().unwrap().to_string();
    let p =
        writer.post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": head, "title": message})).await;
    let id = p.json["id"].as_str().unwrap();
    let r = integrator.post(&format!("/v1/repos/acme/web/proposals/{id}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
    r.json["outcome"]["new_head"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blame_over_http_and_grpc() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let bob = user_with_role(&srv, "user:bob", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let reader = user_with_role(&srv, "user:rita", "acme/web", "reader").await;

    let first = land(&alice, &carol, "w1", "notes.txt", "alpha\nbeta\ngamma\n", "create notes").await;
    let second = land(&bob, &carol, "w2", "notes.txt", "alpha\nBETA\ngamma\n", "shout beta").await;

    let r = reader.get("/v1/repos/acme/web/blame?path=notes.txt").await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["rev"], second.as_str(), "an empty rev means the default stream");
    let lines = r.json["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["text"], "alpha");
    assert_eq!(lines[0]["changeset"], first.as_str());
    assert_eq!(lines[0]["author"], "user:alice");
    assert_eq!(lines[1]["text"], "BETA");
    assert_eq!(lines[1]["changeset"], second.as_str());
    assert_eq!(lines[1]["author"], "user:bob");
    assert_eq!(lines[1]["summary"], "shout beta");
    assert_eq!(r.json["truncated"], false);

    // A revision, a range, and the error cases.
    let old =
        reader.get(&format!("/v1/repos/acme/web/blame?path=notes.txt&rev={first}&first_line=2&last_line=2")).await;
    assert_eq!(old.status, 200, "{}", old.json);
    assert_eq!(old.json["lines"].as_array().unwrap().len(), 1);
    assert_eq!(old.json["lines"][0]["text"], "beta");
    assert_eq!(reader.get("/v1/repos/acme/web/blame?path=missing.txt").await.status, 404);
    assert_eq!(reader.get("/v1/repos/acme/web/blame").await.status, 400, "path is required");
    assert_eq!(reader.get("/v1/repos/acme/web/blame?path=notes.txt&first_line=3&last_line=1").await.status, 400);
    // No role, no blame (and no hint that the repository exists).
    let outsider = user_with_role(&srv, "user:eve", "other/*", "reader").await;
    assert_eq!(outsider.get("/v1/repos/acme/web/blame?path=notes.txt").await.status, 404);

    // The same over gRPC.
    let ch = tonic::transport::Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap();
    let mut req = Request::new(v1::BlameRequest {
        repo: "acme/web".into(),
        path: "notes.txt".into(),
        rev: "main".into(),
        ..Default::default()
    });
    req.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {}", reader.token)).unwrap());
    let g = HistoryServiceClient::new(ch).blame(req).await.unwrap().into_inner();
    assert_eq!(g.lines.len(), 3);
    assert_eq!(g.lines[1].changeset, second);
    assert_eq!(g.lines[1].timestamp_ms, lines[1]["timestamp_ms"].as_i64().unwrap());

    // The call is part of the generated, checked API description.
    let spec: Value = srv.client("").get("/openapi.json").await.json;
    assert_eq!(spec["paths"]["/v1/repos/{owner}/{rname}/blame"]["get"]["operationId"], "history.blame");
    srv.stop().await;
}
