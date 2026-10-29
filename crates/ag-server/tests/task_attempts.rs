//! Durable attempt admission, keyed replay and operator recovery over HTTP.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn attempts_are_counted_once_and_an_operator_can_extend_the_same_task() {
    let server = TestServer::start().await;
    let admin = server.admin();
    let repo = "acme/attempts";
    let base = format!("/v1/repos/{repo}");
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let creator = user_with_role(&server, "user:creator", repo, "writer").await;
    let reader = user_with_role(&server, "user:reader", repo, "reader").await;
    let key = ag_crypto::KeyPair::from_seed([17; 32]);
    assert_eq!(
        admin
            .post(&format!("{base}/agents"), json!({"name":"retry-bot","public_key":key.public().to_hex()}))
            .await
            .status,
        200
    );
    let created = creator.post(&format!("{base}/tasks"), json!({"title":"Preserve this task", "description":"Original instructions", "max_attempts":1,"idempotency_key":"create-once"})).await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["max_attempts"], 1);
    let task = created.json["id"].as_str().unwrap();
    let run_body = json!({"agent":"retry-bot", "task_id":task,"idempotency_key":"run-once"});
    let started = admin.post(&format!("{base}/runs"), run_body.clone()).await;
    assert_eq!(started.status, 200, "{}", started.json);
    assert_eq!(admin.post(&format!("{base}/runs"), run_body).await.json, started.json);
    let current = creator.get(&format!("{base}/tasks/{task}")).await;
    assert_eq!(current.json["attempts_started"], 1);
    assert_eq!(current.json["ready"], false);
    let run = started.json["run_id"].as_str().unwrap();
    assert_eq!(
        admin
            .post(&format!("{base}/runs/{run}/complete"), json!({"status":"failed","summary":"First attempt failed"}))
            .await
            .status,
        200
    );
    let denied = admin.post(&format!("{base}/runs"), json!({"agent":"retry-bot","task_id":task})).await;
    assert_eq!(denied.status, 412, "{}", denied.json);
    let path = format!("{base}/tasks/{task}/attempt-limit");
    let update = json!({"max_attempts":2,"expected_max_attempts":1,"idempotency_key":"extend-once"});
    assert_eq!(reader.call("PATCH", &path, Some(update.clone()), &[]).await.status, 403);
    assert_eq!(creator.call("PATCH", &path, Some(update.clone()), &[]).await.status, 412);
    let extended = admin.call("PATCH", &path, Some(update.clone()), &[]).await;
    assert_eq!(extended.status, 200, "{}", extended.json);
    assert_eq!(extended.json["attempts_started"], 1);
    assert_eq!(extended.json["description"], "Original instructions");
    assert_eq!(admin.call("PATCH", &path, Some(update), &[]).await.json, extended.json);
    assert_eq!(
        admin.call("PATCH", &path, Some(json!({"max_attempts":3,"expected_max_attempts":1})), &[]).await.status,
        412
    );
    let stale_finish = admin
        .post(&format!("{base}/tasks/{task}/finish"), json!({"state":"cancelled", "expected_max_attempts":1}))
        .await;
    assert_eq!(stale_finish.status, 412, "{}", stale_finish.json);
    assert_eq!(creator.get(&format!("{base}/tasks/{task}")).await.json["state"], "in_progress");
    let second = admin.post(&format!("{base}/runs"), json!({"agent":"retry-bot","task_id":task})).await;
    assert_eq!(second.status, 200, "{}", second.json);
    assert_ne!(second.json["run_id"], started.json["run_id"]);
    assert_eq!(creator.get(&format!("{base}/tasks/{task}")).await.json["attempts_started"], 2);
    let stale_run_finish = admin
        .post(
            &format!("{base}/tasks/{task}/finish"),
            json!({"state":"cancelled", "expected_max_attempts":2, "expected_attempts_started":1}),
        )
        .await;
    assert_eq!(stale_run_finish.status, 412, "{}", stale_run_finish.json);
    assert_eq!(creator.get(&format!("{base}/tasks/{task}")).await.json["state"], "in_progress");
    let second_run = second.json["run_id"].as_str().unwrap();
    assert_eq!(
        admin.post(&format!("{base}/runs/{second_run}/complete"), json!({"status":"completed"})).await.status,
        200
    );
    let current_finish = admin
        .post(&format!("{base}/tasks/{task}/finish"), json!({"state":"done", "expected_attempts_started":2}))
        .await;
    assert_eq!(current_finish.status, 200, "{}", current_finish.json);
    assert_eq!(current_finish.json["state"], "done");
    server.stop().await;
}
