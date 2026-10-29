//! Virtual sync binds resolutions to the branch revision the client inspected.
mod common;
use ag_protocol::v1::{self, workspace_service_client::WorkspaceServiceClient};
use common::*;
use serde_json::json;
use tonic::{Code, Request};
fn authorized<T>(token: &str, body: T) -> Request<T> {
    let mut request = Request::new(body);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_rejects_a_stale_target_and_reports_the_exact_branch_used() {
    let server = TestServer::start().await;
    let admin = server.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/pinned"})).await.status, 200);
    assert_eq!(
        admin.post("/v1/repos/acme/pinned/workspaces", json!({"name":"work","stream":"main"})).await.status,
        200
    );
    let sync = "/v1/repos/acme/pinned/workspaces/work/sync";
    let stale = admin.post(sync, json!({"expected_stream_head":"0".repeat(64)})).await;
    assert_eq!(stale.status, 412, "{}", stale.json);
    let workspace = admin.get("/v1/repos/acme/pinned/workspaces/work").await;
    assert_eq!(workspace.json["head"], "");
    let (head, objects) =
        Client::new("acme/pinned").commit(vec![], "user:admin", "first", &[("file", b"content")], 1_700_000_000_000);
    upload(&admin, "acme/pinned", &objects).await;
    let published = admin
        .post(
            "/v1/repos/acme/pinned/proposals",
            json!({"stream":"main","head":head.to_string(),"title":"publish initial"}),
        )
        .await;
    assert_eq!(published.status, 200, "{}", published.json);
    let moved = admin
        .post(
            "/v1/repos/acme/pinned/streams/cas",
            json!({"stream":"main","expected_version":0,"new_head":head.to_string()}),
        )
        .await;
    assert_eq!(moved.status, 200, "{}", moved.json);
    let synced = admin.post(sync, json!({"expected_stream_head":head.to_string()})).await;
    assert_eq!(synced.status, 200, "{}", synced.json);
    assert_eq!(synced.json["head"], head.to_string());
    assert_eq!(synced.json["stream_head"], head.to_string());
    let mut grpc = WorkspaceServiceClient::connect(server.base.clone()).await.unwrap();
    let stale = v1::SyncWorkspaceRequest {
        repo: "acme/pinned".into(),
        name: "work".into(),
        expected_stream_head: "0".repeat(64),
        ..Default::default()
    };
    assert_eq!(grpc.sync(authorized(&admin.token, stale)).await.unwrap_err().code(), Code::FailedPrecondition);
    let pinned = v1::SyncWorkspaceRequest {
        repo: "acme/pinned".into(),
        name: "work".into(),
        expected_stream_head: head.to_string(),
        ..Default::default()
    };
    assert_eq!(grpc.sync(authorized(&admin.token, pinned)).await.unwrap().into_inner().stream_head, head.to_string());
    server.stop().await;
}
