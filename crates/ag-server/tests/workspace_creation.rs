//! Failed workspace creation must have no persistent side effects.
mod common;
use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_start_revision_does_not_reserve_the_workspace_name() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/web"})).await.status, 200);
    let path = "/v1/repos/acme/web/workspaces";
    let bad = admin.post(path, json!({"name":"retry-me", "stream":"main", "from_rev":"does-not-exist"})).await;
    assert_eq!(bad.status, 404, "{}", bad.json);
    let list = admin.get(path).await;
    assert!(!list.json["workspaces"].as_array().unwrap().iter().any(|row| row["name"] == "retry-me"), "{}", list.json);
    let audit = admin.get("/v1/repos/acme/web/audit?contains=retry-me").await;
    assert_eq!(audit.json["entries"].as_array().unwrap().len(), 0);
    let retried = admin.post(path, json!({"name":"retry-me", "stream":"main"})).await;
    assert_eq!(retried.status, 200, "{}", retried.json);
    assert_eq!(retried.json["base"], "");
    assert_eq!(retried.json["head"], "");
    srv.stop().await;
}
