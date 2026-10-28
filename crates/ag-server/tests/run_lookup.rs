//! Exact run lookup remains available after the session finishes.
mod common;
use ag_crypto::KeyPair;
use common::*;
use serde_json::json;

#[tokio::test]
async fn readers_can_inspect_historical_runs_but_unrelated_principals_cannot() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name":"acme/runs"})).await;
    let key = KeyPair::generate().unwrap();
    assert_eq!(
        admin
            .post(
                "/v1/repos/acme/runs/agents",
                json!({"name":"worker","public_key":key.public().to_hex(),"role":"agent"})
            )
            .await
            .status,
        200
    );
    let run = admin.post("/v1/repos/acme/runs/runs", json!({"agent":"worker"})).await;
    assert_eq!(run.status, 200, "{}", run.json);
    let id = run.json["run_id"].as_str().unwrap();
    let path = format!("/v1/repos/acme/runs/runs/{id}");
    let reader = user_with_role(&srv, "user:reader", "acme/runs", "reader").await;
    assert_eq!(reader.get(&path).await.json["status"], "running");
    assert_eq!(
        admin
            .post(&format!("{path}/complete"), json!({"status":"failed","summary":"Compiler exited with status 2"}))
            .await
            .status,
        200
    );
    let finished = reader.get(&path).await;
    assert_eq!(finished.status, 200);
    assert_eq!(finished.json["summary"], "Compiler exited with status 2");
    assert_eq!(finished.json["status"], "failed");
    let outsider = user_with_role(&srv, "user:outsider", "other/*", "reader").await;
    assert_eq!(outsider.get(&path).await.status, 404);
    assert_eq!(srv.client("").get(&path).await.status, 401);
    assert_eq!(reader.get("/v1/repos/acme/runs/runs/run_missing").await.status, 404);
    srv.stop().await;
}
