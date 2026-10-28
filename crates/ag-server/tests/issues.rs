//! Issue lifecycle over real HTTP, including concurrency and repository permissions.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn issue_lifecycle_preserves_discussion_and_rejects_stale_edits() {
    let srv = TestServer::start().await;
    let path = "/v1/repos/acme/issues/issues";
    assert_eq!(srv.admin().post("/v1/repos", json!({"name":"acme/issues"})).await.status, 200);
    let writer = user_with_role(&srv, "user:author", "acme/issues", "writer").await;
    let other = user_with_role(&srv, "user:other", "acme/issues", "writer").await;
    let reader = user_with_role(&srv, "user:reader", "acme/issues", "reader").await;
    let outsider = user_with_role(&srv, "user:outsider", "elsewhere/*", "reader").await;
    let body = json!({"title":"Import project", "body":"Keep discussion", "idempotency_key":"issue-create-1"});
    let created = writer.post(path, body.clone()).await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(writer.post(path, body.clone()).await.json, created.json);
    assert_eq!(reader.post(path, body).await.status, 403);
    let issue = format!("{path}/{}", created.json["number"].as_u64().unwrap());
    assert_eq!(outsider.get(&issue).await.status, 404);
    assert_eq!(reader.get(&issue).await.json["title"], "Import project");
    let comment = json!({"body":"Ready to migrate", "idempotency_key":"issue-comment-1"});
    let commented = other.post(&format!("{issue}/comments"), comment.clone()).await;
    assert_eq!(commented.status, 200, "{}", commented.json);
    assert_eq!(other.post(&format!("{issue}/comments"), comment).await.json, commented.json);
    let mut edit = json!({"title":"Import project", "body":"Keep discussion", "state":"closed", "expected_version":1});
    assert_eq!(other.call("PUT", &issue, Some(edit.clone()), &[]).await.status, 403);
    let closed = writer.call("PUT", &issue, Some(edit.clone()), &[]).await;
    assert_eq!(closed.status, 200, "{}", closed.json);
    assert_eq!(closed.json["version"], 2);
    assert_eq!(writer.call("PUT", &issue, Some(edit.clone()), &[]).await.status, 412);
    edit["state"] = json!("open");
    edit["expected_version"] = json!(2);
    assert_eq!(srv.admin().call("PUT", &issue, Some(edit), &[]).await.status, 200);
    let activity = reader.get(&format!("{issue}/activity")).await;
    let kinds: Vec<_> =
        activity.json["activity"].as_array().unwrap().iter().map(|v| v["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["created", "commented", "closed", "reopened"]);
    assert_eq!(reader.get(&format!("{path}?state=closed")).await.json["issues"], json!([]));
    assert_eq!(reader.get(&format!("{path}?query=Import")).await.json["issues"].as_array().unwrap().len(), 1);
    assert_eq!(writer.post(&format!("{path}/999/comments"), json!({"body":"Missing"})).await.status, 404);
    assert_eq!(writer.post(path, json!({"title":"Second issue"})).await.status, 200);
    let page = reader.get(&format!("{path}?page_size=1")).await;
    let token = page.json["next_page_token"].as_str().unwrap();
    assert!(!token.is_empty());
    let next = reader.get(&format!("{path}?page_size=1&page_token={token}")).await;
    assert_eq!(next.status, 200, "{}", next.json);
    assert_eq!(next.json["issues"][0]["title"], "Import project");
    assert_eq!(reader.get(&format!("{path}?state=closed&page_token={token}")).await.status, 400);
    let mut task = json!({"title":"Implement the request", "scope":["src/**"], "source_issue":1, "expected_issue_version":2, "idempotency_key":"source-task-1"});
    let tasks = "/v1/repos/acme/issues/tasks";
    assert_eq!(other.post(tasks, task.clone()).await.status, 403);
    assert_eq!(writer.post(tasks, task.clone()).await.status, 412);
    assert_eq!(writer.get(tasks).await.json["tasks"], json!([]));
    task["expected_issue_version"] = json!(3);
    let queued = writer.post(tasks, task.clone()).await;
    assert_eq!(queued.status, 200, "{}", queued.json);
    assert_eq!(writer.post(tasks, task).await.json, queued.json);
    let linked = reader.get(&issue).await;
    assert_eq!(linked.json["task_id"], queued.json["id"]);
    assert_eq!(linked.json["version"], 4);
    assert_eq!(writer.get(tasks).await.json["tasks"].as_array().unwrap().len(), 1);
    srv.stop().await;
}
