//! Follow-up source links are validated and committed with creation, never inferred from text.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn follow_up_creation_is_atomic_and_keeps_the_source_run_independent() {
    let s = TestServer::start().await;
    let admin = s.admin();
    let repo = "acme/lineage";
    let base = "/v1/repos/acme/lineage";
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let key = ag_crypto::KeyPair::generate().unwrap().public().to_hex();
    assert_eq!(admin.post(&format!("{base}/agents"), json!({"name":"bot","public_key":key})).await.status, 200);
    let source = admin.post(&format!("{base}/tasks"), json!({"title":"original","scope":["src/**"]})).await;
    let source_id = source.json["id"].as_str().unwrap();
    let run = admin.post(&format!("{base}/runs"), json!({"agent":"bot","task_id":source_id})).await;
    let run_id = run.json["run_id"].as_str().unwrap();
    assert_eq!(admin.post(&format!("{base}/runs/{run_id}/complete"), json!({"status":"failed"})).await.status, 200);
    let source_before = admin.get(&format!("{base}/tasks/{source_id}")).await.json;
    let body = json!({"title":"continue deliberately","source_task":source_id,"source_run":run_id,"idempotency_key":"follow-up"});
    let created = admin.post(&format!("{base}/tasks"), body.clone()).await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["source_task"], source_id);
    assert_eq!(created.json["source_run"], run_id);
    assert_eq!(created.json["owner"], "");
    assert_eq!(created.json["state"], "open");
    assert_eq!(created.json["scope"], json!([]));
    assert_eq!(created.json["depends_on"], json!([]));
    assert_eq!(admin.post(&format!("{base}/tasks"), body.clone()).await.json, created.json);
    assert_eq!(admin.get(&format!("{base}/tasks/{source_id}")).await.json, source_before);
    let follow_id = created.json["id"].as_str().unwrap();
    assert_eq!(admin.get(&format!("{base}/tasks/{follow_id}")).await.json, created.json);
    let collision = json!({"title":"continue deliberately", "idempotency_key":"follow-up",
        "depends_on":[format!("\0source:{}", json!([source_id, run_id]))]});
    assert_eq!(admin.post(&format!("{base}/tasks"), collision).await.status, 422);
    let mut reused = body.clone();
    reused["source_run"] = json!("");
    assert_eq!(admin.post(&format!("{base}/tasks"), reused).await.status, 422);
    let invalid = json!({"title":"invalid source","source_task":follow_id,"source_run":run_id});
    assert_eq!(admin.post(&format!("{base}/tasks"), invalid).await.status, 412);
    assert_eq!(
        admin.post(&format!("{base}/tasks"), json!({"title":"missing task","source_run":run_id})).await.status,
        412
    );
    assert_eq!(
        admin.post(&format!("{base}/tasks"), json!({"title":"foreign","source_task":"task_missing"})).await.status,
        404
    );
    let all = admin.get(&format!("{base}/tasks")).await;
    assert_eq!(all.json["tasks"].as_array().unwrap().len(), 2);
    let reader = user_with_role(&s, "user:reader", repo, "reader").await;
    assert_eq!(reader.post(&format!("{base}/tasks"), body).await.status, 403);
    s.stop().await;
}
