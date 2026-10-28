//! Bounded operator output survives completion and cannot be resurrected after deletion.
mod common;
use ag_crypto::KeyPair;
use common::*;
use serde_json::json;

#[tokio::test]
async fn output_is_bounded_operator_only_and_deletion_blocks_retries() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let repo = "acme/output";
    let base = "/v1/repos/acme/output";
    admin.post("/v1/repos", json!({"name":repo})).await;
    admin
        .post(
            &format!("{base}/agents"),
            json!({"name":"bot","public_key":KeyPair::generate().unwrap().public().to_hex()}),
        )
        .await;
    let run = admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await;
    let id = run.json["run_id"].as_str().unwrap();
    let path = format!("{base}/runs/{id}/output");
    let writer = user_with_role(&srv, "user:writer", repo, "writer").await;
    let reader = user_with_role(&srv, "user:reader", repo, "reader").await;
    let body = json!({"output":{"sequence":1,"stdout":"first line\n","stderr":"failure detail","stdout_bytes":11,"stderr_bytes":14}});
    assert_eq!(writer.call("PUT", &path, Some(body.clone()), &[]).await.status, 403);
    let saved = admin.call("PUT", &path, Some(body.clone()), &[]).await;
    assert_eq!(saved.status, 200, "{}", saved.json);
    assert_eq!(admin.call("PUT", &path, Some(body.clone()), &[]).await.json, saved.json);
    let huge = json!({"output":{"sequence":2,"stdout":"x".repeat(32769)}});
    assert_eq!(admin.call("PUT", &path, Some(huge), &[]).await.status, 400);
    admin.post(&format!("{base}/runs/{id}/complete"), json!({"status":"failed"})).await;
    assert_eq!(reader.get(&path).await.json["stderr"], "failure detail");
    assert_eq!(reader.call("DELETE", &path, None, &[]).await.status, 403);
    assert_eq!(admin.call("DELETE", &path, None, &[]).await.status, 200);
    assert_eq!(reader.get(&path).await.json["available"], false);
    assert_eq!(admin.call("PUT", &path, Some(body), &[]).await.status, 412);
    let active = admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await;
    let active_path = format!("{base}/runs/{}/output", active.json["run_id"].as_str().unwrap());
    assert_eq!(admin.call("DELETE", &active_path, None, &[]).await.status, 200);
    assert_eq!(
        admin
            .call("PUT", &active_path, Some(json!({"output":{"sequence":2,"stdout":"late upload"}})), &[])
            .await
            .status,
        412
    );
    srv.stop().await;
}
