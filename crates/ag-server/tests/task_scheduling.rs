//! Operators control task launches; readers see the persisted decision.
mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn only_operators_change_scheduling_and_stale_decisions_are_rejected() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name":"acme/work"})).await;
    let writer = user_with_role(&srv, "user:writer", "acme/work", "writer").await;
    let reader = user_with_role(&srv, "user:reader", "acme/work", "reader").await;
    let operator = user_with_role(&srv, "user:operator", "acme/work", "delegator").await;
    let path = "/v1/repos/acme/work/tasks/scheduling";
    assert_eq!(reader.get(path).await.json["paused"], false);
    let pause = json!({"paused":true,"reason":"Deploying executor update","expected_version":0});
    assert_eq!(writer.call("PUT", path, Some(pause.clone()), &[]).await.status, 403);
    let result = operator.call("PUT", path, Some(pause.clone()), &[]).await;
    assert_eq!(result.status, 200, "{}", result.json);
    assert_eq!(result.json["version"], 1);
    assert_eq!(operator.call("PUT", path, Some(pause), &[]).await.status, 412);
    assert_eq!(reader.get(path).await.json["reason"], "Deploying executor update");
    let resume = json!({"paused":false,"expected_version":1});
    assert_eq!(operator.call("PUT", path, Some(resume), &[]).await.status, 200);
    assert_eq!(reader.get(path).await.json["paused"], false);
    srv.stop().await;
}
