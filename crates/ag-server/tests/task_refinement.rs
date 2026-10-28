//! Task instructions are editable before first execution, with content CAS and keyed retries.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn refinement_is_atomic_authorized_and_replay_safe() {
    let s = TestServer::start().await;
    let admin = s.admin();
    let repo = "acme/refinement";
    let base = "/v1/repos/acme/refinement";
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let writer = user_with_role(&s, "user:alice", repo, "writer").await;
    let other = user_with_role(&s, "user:bob", repo, "writer").await;
    let reader = user_with_role(&s, "user:reader", repo, "reader").await;
    let expected = json!({"title":"Initial","description":"Before","scope":["src/**"]});
    let task = writer.post(&format!("{base}/tasks"), expected.clone()).await;
    assert_eq!(task.status, 200);
    let id = task.json["id"].as_str().unwrap();
    let path = format!("{base}/tasks/{id}");
    let content = json!({"title":"Refined λ","description":"Concrete result","scope":["lib/**"]});
    let body = json!({"content":content,"expected":expected,"idempotency_key":"refine-once"});
    for caller in [&other, &reader] {
        assert_eq!(caller.call("PATCH", &path, Some(body.clone()), &[]).await.status, 403);
    }
    let (left, right) = tokio::join!(
        writer.call("PATCH", &path, Some(body.clone()), &[]),
        writer.call("PATCH", &path, Some(body.clone()), &[])
    );
    assert_eq!(left.status, 200, "{}", left.json);
    assert_eq!(right.status, 200, "{}", right.json);
    assert_eq!(left.json, right.json);
    assert_eq!(writer.get(&path).await.json, left.json);
    assert_eq!(left.json["title"], "Refined λ");
    let stale = json!({"content":content,"expected":expected});
    assert_eq!(writer.call("PATCH", &path, Some(stale), &[]).await.status, 412);
    let reused = json!({"content":expected,"expected":content,"idempotency_key":"refine-once"});
    assert_eq!(writer.call("PATCH", &path, Some(reused), &[]).await.status, 422);
    assert_eq!(writer.call("PATCH", &path, Some(json!({"content":content})), &[]).await.status, 400);
    let invalid = json!({"content":{"title":" ","scope":["["]},"expected":content});
    assert_eq!(writer.call("PATCH", &path, Some(invalid), &[]).await.status, 400);
    assert_eq!(writer.get(&path).await.json, left.json);
    let key = ag_crypto::KeyPair::generate().unwrap().public().to_hex();
    assert_eq!(admin.post(&format!("{base}/agents"), json!({"name":"bot","public_key":key})).await.status, 200);
    let run = admin.post(&format!("{base}/runs"), json!({"agent":"bot","task_id":id})).await;
    assert_eq!(run.status, 200, "{}", run.json);
    let edit = json!({"content":expected,"expected":content});
    assert_eq!(admin.call("PATCH", &path, Some(edit), &[]).await.status, 412);
    // A lost response can still be replayed after execution starts.
    assert_eq!(writer.call("PATCH", &path, Some(body), &[]).await.json, left.json);
    let handle = s.app.repos.handle(repo).unwrap();
    let audit = handle.meta().read(|tx| tx.audit_list(Some(id), 100)).unwrap();
    assert_eq!(audit.iter().filter(|entry| entry.action == "task.update").count(), 1);
    s.stop().await;
}
