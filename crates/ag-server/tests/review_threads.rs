//! Real-server lifecycle for revision-bound review discussions.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn line_threads_keep_their_revision_and_serialize_resolution() {
    let server = TestServer::start().await;
    let repo = "acme/threads";
    let base = "/v1/repos/acme/threads";
    let admin = server.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let alice = user_with_role(&server, "user:alice", repo, "writer").await;
    let bob = user_with_role(&server, "user:bob", repo, "writer").await;
    let reader = user_with_role(&server, "user:reader", repo, "reader").await;
    let client = Client::new(repo);
    let (head, objects) =
        client.commit(vec![], "user:alice", "change", &[("file.txt", b"first\nsecond\n")], 1_700_000_000_000);
    upload(&alice, repo, &objects).await;
    let proposal = alice
        .post(&format!("{base}/proposals"), json!({"stream":"main","head":head.to_string(),"title":"Review lines"}))
        .await;
    assert_eq!(proposal.status, 200, "{}", proposal.json);
    let id = proposal.json["id"].as_str().unwrap();
    let route = format!("{base}/proposals/{id}/threads");
    let body = json!({"body":"Can we explain this branch?","expected_head":head.to_string(),"anchor":{"base":"","head":head.to_string(),"path":"file.txt","side":"new","line":1,"text":"spoofed by client"},"idempotency_key":"first-thread"});
    let thread = alice.post(&route, body.clone()).await;
    assert_eq!(thread.status, 200, "{}", thread.json);
    let thread_id = thread.json["id"].as_str().unwrap();
    assert_eq!(alice.post(&route, body.clone()).await.json, thread.json);
    assert_eq!(reader.post(&route, body.clone()).await.status, 403);
    let bad = alice.post(&route, json!({"body":"wrong line","expected_head":head.to_string(),"anchor":{"base":"","head":head.to_string(),"path":"file.txt","side":"new","line":200}})).await;
    assert_eq!(bad.status, 400, "{}", bad.json);
    let mut bad_base = body.clone();
    bad_base["anchor"]["base"] = json!("ff".repeat(32));
    bad_base["idempotency_key"] = json!("bad-base");
    assert_eq!(alice.post(&route, bad_base).await.status, 400);
    let mut reused = body.clone();
    reused["anchor"]["line"] = json!(2);
    assert_eq!(alice.post(&route, reused).await.status, 422);
    let comments = format!("{route}/{thread_id}/comments");
    let reply_body = json!({"body":"This covers the fallback.","idempotency_key":"reply-once"});
    let reply = bob.post(&comments, reply_body.clone()).await;
    assert_eq!(reply.status, 200, "{}", reply.json);
    assert_eq!(bob.post(&comments, reply_body).await.json, reply.json);
    assert_eq!(reader.post(&comments, json!({"body":"forbidden"})).await.status, 403);
    let resolution = format!("{route}/{thread_id}");
    assert_eq!(
        bob.call("PATCH", &resolution, Some(json!({"resolved":true,"expected_version":2})), &[]).await.status,
        403
    );
    let resolved = alice
        .call(
            "PATCH",
            &resolution,
            Some(json!({"resolved":true,"expected_version":2,"idempotency_key":"resolve-once"})),
            &[],
        )
        .await;
    assert_eq!(resolved.status, 200, "{}", resolved.json);
    assert_eq!(resolved.json["resolved"], true);
    assert_eq!(resolved.json["version"], 3);
    assert_eq!(
        alice.call("PATCH", &resolution, Some(json!({"resolved":false,"expected_version":1})), &[]).await.status,
        412
    );
    let reopened = alice.call("PATCH", &resolution, Some(json!({"resolved":false,"expected_version":3})), &[]).await;
    assert_eq!(reopened.status, 200, "{}", reopened.json);
    let (left, right) = tokio::join!(
        alice.call(
            "PATCH",
            &resolution,
            Some(json!({"resolved":true,"expected_version":4,"idempotency_key":"race-a"})),
            &[]
        ),
        admin.call(
            "PATCH",
            &resolution,
            Some(json!({"resolved":true,"expected_version":4,"idempotency_key":"race-b"})),
            &[]
        )
    );
    let mut statuses = vec![left.status, right.status];
    statuses.sort();
    assert_eq!(statuses, vec![200, 412]);
    assert_eq!(bob.post(&comments, json!({"body":"cannot reply until reopened"})).await.status, 400);
    let (next, next_objects) =
        client.commit(vec![], "user:alice", "replacement", &[("file.txt", b"replacement\n")], 1_700_000_001_000);
    upload(&alice, repo, &next_objects).await;
    let moved =
        alice.call("PATCH", &format!("{base}/proposals/{id}"), Some(json!({"new_head":next.to_string()})), &[]).await;
    assert_eq!(moved.status, 200, "{}", moved.json);
    assert_eq!(
        alice.post(&route, body.clone()).await.json,
        thread.json,
        "accepted creation replays after a head change"
    );
    let mut stale_body = body.clone();
    stale_body["idempotency_key"] = json!("stale-new-thread");
    assert_eq!(alice.post(&route, stale_body).await.status, 412);
    let gc = admin.post(&format!("{base}/gc"), json!({"grace_ms":1,"reflog_retention_ms":1})).await;
    assert_eq!(gc.status, 200, "{}", gc.json);
    assert_eq!(gc.json["reachable_changesets"], 2, "both current head and old thread source remain roots");
    let source = reader.get(&format!("{base}/files/file.txt?rev={head}")).await;
    assert_eq!(source.status, 200);
    assert_eq!(source.body, b"first\nsecond\n");
    let list = reader.get(&route).await;
    assert_eq!(list.status, 200);
    assert_eq!(list.json["threads"].as_array().unwrap().len(), 1);
    assert_eq!(list.json["threads"][0]["anchor"]["head"], head.to_string());
    assert_eq!(list.json["threads"][0]["anchor"]["text"], "first");
    let replies = reader.get(&comments).await;
    assert_eq!(replies.json["comments"].as_array().unwrap().len(), 1);
    let detail = reader.get(&format!("{base}/proposals/{id}")).await;
    assert!(detail.json["reviews"].as_array().unwrap().is_empty(), "discussion must not create approval decisions");
    server.stop().await;
}
