//! Operator state changes and new-run admission share repository transactions.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn operator_controls_are_replay_safe_and_do_not_end_running_work() {
    let s = TestServer::start().await;
    let admin = s.admin();
    let repo = "acme/agents";
    let base = "/v1/repos/acme/agents";
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let key = ag_crypto::KeyPair::generate().unwrap().public().to_hex();
    assert_eq!(admin.post(&format!("{base}/agents"), json!({"name":"bot","public_key":key})).await.status, 200);
    let running = admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await;
    assert_eq!(running.status, 200);
    let path = format!("{base}/agents/bot");
    let disable = json!({"disabled":true,"expected_disabled":false,"idempotency_key":"disable-once"});
    let writer = user_with_role(&s, "user:writer", repo, "writer").await;
    assert_eq!(writer.call("PATCH", &path, Some(disable.clone()), &[]).await.status, 403);
    let other_agent = user_with_role(&s, "agent:other", repo, "delegator").await;
    assert_eq!(other_agent.call("PATCH", &path, Some(disable.clone()), &[]).await.status, 403);
    let (left, right) = tokio::join!(
        admin.call("PATCH", &path, Some(disable.clone()), &[]),
        admin.call("PATCH", &path, Some(disable.clone()), &[])
    );
    assert_eq!(left.status, 200, "{}", left.json);
    assert_eq!(right.status, 200, "{}", right.json);
    assert_eq!(left.json, right.json);
    assert_eq!(left.json["disabled"], true);
    assert_eq!(admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await.status, 412);
    let run = admin.get(&format!("{base}/runs/{}", running.json["run_id"].as_str().unwrap())).await;
    assert_eq!(run.json["status"], "running");
    let stale = admin.call("PATCH", &path, Some(json!({"disabled":false,"expected_disabled":false})), &[]).await;
    assert_eq!(stale.status, 412);
    let reused = admin
        .call(
            "PATCH",
            &path,
            Some(json!({"disabled":false,"expected_disabled":true,"idempotency_key":"disable-once"})),
            &[],
        )
        .await;
    assert_eq!(reused.status, 422);
    let enabled = admin.call("PATCH", &path, Some(json!({"disabled":false,"expected_disabled":true})), &[]).await;
    assert_eq!(enabled.status, 200, "{}", enabled.json);
    assert_eq!(admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await.status, 200);
    let meta = s.app.repos.handle(repo).unwrap();
    let audit = meta.meta().read(|tx| tx.audit_list(Some("bot"), 100)).unwrap();
    assert_eq!(audit.iter().filter(|entry| entry.action == "agent.disable").count(), 1);
    assert_eq!(audit.iter().filter(|entry| entry.action == "agent.enable").count(), 1);
    s.stop().await;
}
