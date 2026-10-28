//! Discussion is separate from approval, with head-bound and idempotent writes.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn authors_can_discuss_without_approval_rights_and_retries_do_not_duplicate() {
    let server = TestServer::start().await;
    let repo = "acme/discussion";
    let base = "/v1/repos/acme/discussion";
    let admin = server.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let alice = user_with_role(&server, "user:alice", repo, "writer").await;
    let reader = user_with_role(&server, "user:reader", repo, "reader").await;
    let client = Client::new(repo);
    let (head, objects) = client.commit(vec![], "user:alice", "change", &[("file.txt", b"first\n")], 1_700_000_000_000);
    upload(&alice, repo, &objects).await;
    let proposal = alice
        .post(
            &format!("{base}/proposals"),
            json!({"stream":"main", "head":head.to_string(), "title":"Discuss this change"}),
        )
        .await;
    assert_eq!(proposal.status, 200, "{}", proposal.json);
    let id = proposal.json["id"].as_str().unwrap();
    let route = format!("{base}/proposals/{id}/comments");
    let body = json!({"body":"I kept this explicit for the caller.", "expected_head":head.to_string(), "idempotency_key":"author-comment"});
    let comment = alice.post(&route, body.clone()).await;
    assert_eq!(comment.status, 200, "{}", comment.json);
    assert_eq!(comment.json["decision"], "comment");
    assert_eq!(alice.post(&route, body.clone()).await.json, comment.json);
    assert_eq!(reader.post(&route, body.clone()).await.status, 403);
    assert_eq!(alice.post(&format!("{base}/proposals/{id}/reviews"), json!({"decision":"approve"})).await.status, 403);
    assert_eq!(alice.post(&route, json!({"body":"", "expected_head":head.to_string()})).await.status, 400);
    assert_eq!(
        alice.post(&route, json!({"body":"x".repeat(16385), "expected_head":head.to_string()})).await.status,
        400
    );
    let stale =
        alice.post(&route, json!({"body":"stale", "expected_head":"00".repeat(32), "idempotency_key":"stale"})).await;
    assert_eq!(stale.status, 412, "{}", stale.json);
    let reused = alice
        .post(&route, json!({"body":"different", "expected_head":head.to_string(), "idempotency_key":"author-comment"}))
        .await;
    assert_eq!(reused.status, 422);
    assert_eq!(reused.json["error"]["code"], "idempotency_key_reused");
    assert_eq!(admin.post(&format!("{base}/proposals/{id}/close"), json!({})).await.status, 200);
    let after = alice.post(&route, json!({"body":"Closed after discussing the alternative.", "expected_head":head.to_string(), "idempotency_key":"closed-comment"})).await;
    assert_eq!(after.status, 200, "{}", after.json);
    let stored = reader.get(&format!("{base}/proposals/{id}")).await;
    assert_eq!(stored.json["reviews"].as_array().unwrap().len(), 2);
    assert!(stored.json["reviews"].as_array().unwrap().iter().all(|row| row["decision"] == "comment"));
    server.stop().await;
}
