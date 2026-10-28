//! Queue assignment is creator/operator-only and atomic with creation.
mod common;
use ag_crypto::KeyPair;
use common::*;
use serde_json::json;

#[tokio::test]
async fn assignment_is_atomic_and_respected_when_a_run_starts() {
    let srv = TestServer::start().await;
    let repo = "acme/assignment";
    let base = "/v1/repos/acme/assignment";
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name":repo})).await;
    for agent in ["first", "chosen"] {
        let key = KeyPair::generate().unwrap();
        assert_eq!(
            admin
                .post(
                    &format!("{base}/agents"),
                    json!({"name":agent,"public_key":key.public().to_hex(),"role":"agent"})
                )
                .await
                .status,
            200
        );
    }
    let writer = user_with_role(&srv, "user:alice", repo, "writer").await;
    let other = user_with_role(&srv, "user:other", repo, "writer").await;
    let created =
        writer.post(&format!("{base}/tasks"), json!({"title":"Assigned from creation","assignee":"chosen"})).await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["owner"], "agent:chosen");
    let id = created.json["id"].as_str().unwrap();
    let assign = format!("{base}/tasks/{id}/assignee");
    let body = json!({"agent":"first","expected_owner":"agent:chosen"});
    assert_eq!(other.call("PUT", &assign, Some(body.clone()), &[]).await.status, 403);
    assert_eq!(writer.call("PUT", &assign, Some(body.clone()), &[]).await.status, 200);
    assert_eq!(writer.call("PUT", &assign, Some(body), &[]).await.status, 412);
    assert_eq!(admin.post(&format!("{base}/runs"), json!({"agent":"chosen","task_id":id})).await.status, 412);
    assert_eq!(admin.post(&format!("{base}/runs"), json!({"agent":"first","task_id":id})).await.status, 200);
    assert_eq!(
        admin.call("PUT", &assign, Some(json!({"agent":"chosen","expected_owner":"agent:first"})), &[]).await.status,
        412
    );
    let issue = writer.post(&format!("{base}/issues"), json!({"title":"No orphan task"})).await;
    let number = issue.json["number"].as_u64().unwrap();
    let bad = writer
        .post(
            &format!("{base}/tasks"),
            json!({"title":"Must roll back","assignee":"missing","source_issue":number,"expected_issue_version":1}),
        )
        .await;
    assert_eq!(bad.status, 404, "{}", bad.json);
    let untouched = writer.get(&format!("{base}/issues/{number}")).await;
    assert_eq!(untouched.json["task_id"], "");
    assert_eq!(untouched.json["version"], 1);
    assert_eq!(writer.get(&format!("{base}/tasks")).await.json["tasks"].as_array().unwrap().len(), 1);
    srv.stop().await;
}
