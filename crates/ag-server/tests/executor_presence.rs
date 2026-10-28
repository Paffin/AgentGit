//! Connectivity reports must not be forgeable by ordinary writers.
mod common;
use ag_crypto::KeyPair;
use common::*;
use serde_json::json;

#[tokio::test]
async fn only_operators_report_and_readers_observe_executor_capacity() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let repo = "acme/presence";
    let base = "/v1/repos/acme/presence";
    admin.post("/v1/repos", json!({"name":repo})).await;
    admin
        .post(
            &format!("{base}/agents"),
            json!({"name":"bot","public_key":KeyPair::generate().unwrap().public().to_hex()}),
        )
        .await;
    let writer = user_with_role(&srv, "user:writer", repo, "writer").await;
    let reader = user_with_role(&srv, "user:reader", repo, "reader").await;
    let endpoint = format!("{base}/agents/bot/executor");
    let report = json!({"name":"local-command","slots":2,"available":1,"ttl_secs":90});
    assert_eq!(writer.call("PUT", &endpoint, Some(report.clone()), &[]).await.status, 403);
    let saved = admin.call("PUT", &endpoint, Some(report), &[]).await;
    assert_eq!(saved.status, 200, "{}", saved.json);
    assert_eq!(saved.json["connected"], true);
    assert_eq!(saved.json["expires_at_ms"].as_i64().unwrap() - saved.json["reported_at_ms"].as_i64().unwrap(), 90000);
    let agents = reader.get(&format!("{base}/agents")).await;
    assert_eq!(agents.json["agents"][0]["executor"]["available"], 1);
    assert_eq!(
        admin
            .call("PUT", &endpoint, Some(json!({"name":"bad","slots":1,"available":2,"ttl_secs":90})), &[])
            .await
            .status,
        400
    );
    srv.stop().await;
}
