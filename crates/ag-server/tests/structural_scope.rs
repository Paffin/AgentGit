//! Only fresh structural sync previews expose a bounded complete candidate scope.
mod common;
use ag_protocol::v1::{self, workspace_service_client::WorkspaceServiceClient};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use common::*;
use serde_json::{Value, json};
use tonic::Request;

async fn checked(http: &Http, path: &str, body: Value) -> Value {
    let response = http.post(path, body).await;
    assert_eq!(response.status, 200, "{}", response.json);
    response.json
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_sync_reports_candidate_scope_at_the_exact_target() {
    let server = TestServer::start().await;
    let admin = server.admin();
    let capabilities = admin.get("/v1/capabilities?supported_versions=agwp.v1").await;
    assert!(
        capabilities.json["features"].as_array().unwrap().iter().any(|value| value == "workspaces.structural_preview")
    );
    let root = "/v1/repos/acme/scope";
    checked(&admin, "/v1/repos", json!({"name":"acme/scope"})).await;
    let (base, objects) =
        Client::new("acme/scope").commit(vec![], "user:admin", "base", &[("base", b"unchanged")], 1_700_000_000_000);
    upload(&admin, "acme/scope", &objects).await;
    checked(&admin, &format!("{root}/proposals"), json!({"stream":"main","head":base.to_string(),"title":"base"}))
        .await;
    checked(
        &admin,
        &format!("{root}/streams/cas"),
        json!({"stream":"main","expected_version":0,"new_head":base.to_string()}),
    )
    .await;
    for name in ["ours", "theirs"] {
        checked(&admin, &format!("{root}/workspaces"), json!({"name":name,"stream":"main"})).await;
    }
    let ours = checked(
        &admin,
        &format!("{root}/workspaces/ours/changes"),
        json!({"changes":[{"path":"clash","op":"put","content":STANDARD.encode("file")}]}),
    )
    .await;
    let theirs = checked(
        &admin,
        &format!("{root}/workspaces/theirs/changes"),
        json!({"changes":[
            {"path":"clash/a","op":"put","content":STANDARD.encode("a")},
            {"path":"clash/b","op":"put","content":STANDARD.encode("b")},
            {"path":"unrelated","op":"put","content":STANDARD.encode("keep")}
        ]}),
    )
    .await;
    checked(&admin, &format!("{root}/proposals"), json!({"stream":"main","head":theirs["head"],"title":"target"}))
        .await;
    checked(
        &admin,
        &format!("{root}/streams/cas"),
        json!({"stream":"main","expected_version":1,"expected_head":base.to_string(),"new_head":theirs["head"]}),
    )
    .await;
    let result =
        checked(&admin, &format!("{root}/workspaces/ours/sync"), json!({"expected_stream_head":theirs["head"]})).await;
    assert_eq!(result["status"], "conflicted");
    assert_eq!(result["stream_head"], theirs["head"]);
    assert_eq!(
        result["conflicts"][0]["resolution_scope"],
        json!({"paths":["clash","clash/a","clash/b"],"complete":true})
    );
    assert_eq!(admin.get(&format!("{root}/workspaces/ours")).await.json["head"], ours["head"]);
    let mut grpc = WorkspaceServiceClient::connect(server.base.clone()).await.unwrap();
    let mut request = Request::new(v1::SyncWorkspaceRequest {
        repo: "acme/scope".into(),
        name: "ours".into(),
        expected_stream_head: theirs["head"].as_str().unwrap().into(),
        ..Default::default()
    });
    request.metadata_mut().insert("authorization", format!("Bearer {}", admin.token).parse().unwrap());
    let response = serde_json::to_value(grpc.sync(request).await.unwrap().into_inner()).unwrap();
    assert_eq!(response["conflicts"], result["conflicts"]);
    let proposal =
        checked(&admin, &format!("{root}/proposals"), json!({"stream":"main","head":ours["head"],"title":"ours"}))
            .await;
    let preview =
        checked(&admin, &format!("{root}/proposals/{}/prepare", proposal["id"].as_str().unwrap()), json!({})).await;
    assert_eq!(preview["conflicts"][0]["kind"], "file_directory");
    assert!(
        preview["conflicts"][0].get("resolution_scope").is_none_or(Value::is_null),
        "scope only belongs to fresh virtual sync"
    );
    server.stop().await;
}
