#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::json;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audit_stream_scoped_reviewer_cannot_approve_other_stream() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"audit/repo"})).await.status, 200);
    assert_eq!(admin.post("/v1/repos/audit/repo/streams", json!({"name":"dev"})).await.status, 200);
    let alice = user_with_role(&srv, "user:alice", "audit/repo", "writer").await;
    let reviewer = user_with_role(&srv, "user:reviewer", "audit/repo", "reviewer").await;
    let token = reviewer
        .post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["audit/repo"],"streams":["dev"]}}))
        .await;
    assert_eq!(token.status, 200, "{}", token.json);
    let narrow = srv.client(token.json["token"].as_str().unwrap());
    let (head, objects) =
        Client::new("audit/repo").commit(vec![], "user:alice", "change", &[("a", b"a")], 1700000000000);
    upload(&alice, "audit/repo", &objects).await;
    let p = alice
        .post("/v1/repos/audit/repo/proposals", json!({"stream":"main","head":head.to_string(),"title":"change"}))
        .await;
    assert_eq!(p.status, 200, "{}", p.json);
    let denied = narrow.get("/v1/repos/audit/repo/streams/head?stream=main").await;
    assert_eq!(denied.status, 403, "{}", denied.json);
    let result = narrow
        .post(
            &format!("/v1/repos/audit/repo/proposals/{}/reviews", p.json["id"].as_str().unwrap()),
            json!({"decision":"approve","expected_head":head.to_string(),"body":"scoped dev review"}),
        )
        .await;
    println!(
        "main head read status={}; main approval status={}, response={}",
        denied.status, result.status, result.json
    );
    assert_eq!(result.status, 403, "stream-scoped reviewer approved an out-of-scope proposal");
    let dev = alice
        .post("/v1/repos/audit/repo/proposals", json!({"stream":"dev","head":head.to_string(),"title":"dev change"}))
        .await;
    assert_eq!(dev.status, 200);
    assert_eq!(
        narrow
            .post(
                &format!("/v1/repos/audit/repo/proposals/{}/reviews", dev.json["id"].as_str().unwrap()),
                json!({"decision":"approve","expected_head":head.to_string()})
            )
            .await
            .status,
        200
    );
    let writer_token =
        alice.post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["audit/repo"],"streams":["dev"]}})).await;
    assert_eq!(writer_token.status, 200);
    let writer = srv.client(writer_token.json["token"].as_str().unwrap());
    let pid = p.json["id"].as_str().unwrap();
    for (method, path, body) in [
        ("PATCH", format!("/v1/repos/audit/repo/proposals/{pid}"), json!({"new_head":head.to_string()})),
        ("POST", format!("/v1/repos/audit/repo/proposals/{pid}/close"), json!({})),
        (
            "POST",
            format!("/v1/repos/audit/repo/proposals/{pid}/comments"),
            json!({"body":"comment","expected_head":head.to_string()}),
        ),
        ("POST", "/v1/repos/audit/repo/queue".into(), json!({"proposal":pid})),
    ] {
        let response = writer.call(method, &path, Some(body), &[]).await;
        assert_eq!(response.status, 403, "{path}: {}", response.json);
    }
    assert_eq!(
        alice.post("/v1/repos/audit/repo/workspaces", json!({"name":"main-work","stream":"main"})).await.status,
        200
    );
    assert_eq!(
        writer.post("/v1/repos/audit/repo/workspaces", json!({"name":"dev-work","stream":"dev"})).await.status,
        200
    );
    for (method, path, body) in [
        (
            "POST",
            "/v1/repos/audit/repo/workspaces/main-work/changes",
            json!({"changes":[{"path":"file","content":"YQ=="}]}),
        ),
        ("POST", "/v1/repos/audit/repo/workspaces/main-work/sync", json!({})),
        ("DELETE", "/v1/repos/audit/repo/workspaces/main-work", json!({})),
    ] {
        assert_eq!(writer.call(method, path, Some(body), &[]).await.status, 403, "{path}");
    }
    // Stream narrowing must not disable repository-wide publication/upload permissions.
    upload(&writer, "audit/repo", &objects).await;
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audit_failed_workspace_create_must_not_leave_a_workspace() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"audit/repo"})).await.status, 200);
    let bad = admin
        .post("/v1/repos/audit/repo/workspaces", json!({"name":"retry-me","stream":"main","from_rev":"does-not-exist"}))
        .await;
    assert!(bad.status >= 400, "{}", bad.json);
    let list = admin.get("/v1/repos/audit/repo/workspaces").await;
    println!("failed create status={}; workspaces={}", bad.status, list.json);
    let retry = admin.post("/v1/repos/audit/repo/workspaces", json!({"name":"retry-me","stream":"main"})).await;
    println!("corrected retry status={}, response={}", retry.status, retry.json);
    srv.stop().await;
    assert_eq!(retry.status, 200, "failed create reserved the workspace name");
}
