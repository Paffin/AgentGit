//! The renamed source is never mistaken for a deleted version by a remote client.
mod common;
use ag_protocol::v1::{self, workspace_service_client::WorkspaceServiceClient};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use common::*;
use serde_json::{Value, json};
use tonic::Request;

async fn checked(http: &Http, path: &str, body: Value) -> Value {
    let result = http.post(path, body).await;
    assert_eq!(result.status, 200, "{}", result.json);
    result.json
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renamed_conflicts_report_exact_sources_for_sync_and_proposal_preparation() {
    let server = TestServer::start().await;
    let admin = server.admin();
    let base = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    let ours = "1\n2\nOURS\n4\n5\n6\n7\n8\n9\n";
    let theirs = "1\n2\nTHEIRS\n4\n5\n6\n7\n8\n9\n";
    for workspace_renames in [true, false] {
        let repo = format!("acme/origins-{workspace_renames}");
        let root = format!("/v1/repos/{repo}");
        checked(&admin, "/v1/repos", json!({"name":repo})).await;
        let (seed, objects) =
            Client::new(&repo).commit(vec![], "user:admin", "base", &[("old.txt", base.as_bytes())], 1_700_000_000_000);
        upload(&admin, &repo, &objects).await;
        checked(&admin, &format!("{root}/proposals"), json!({"stream":"main","head":seed.to_string(),"title":"seed"}))
            .await;
        checked(
            &admin,
            &format!("{root}/streams/cas"),
            json!({"stream":"main","expected_version":0,"new_head":seed.to_string()}),
        )
        .await;
        for name in ["ours", "theirs"] {
            checked(&admin, &format!("{root}/workspaces"), json!({"name":name,"stream":"main"})).await;
        }
        let mut heads = Vec::new();
        for (name, text, renamed) in [("ours", ours, workspace_renames), ("theirs", theirs, !workspace_renames)] {
            let path = if renamed { "new.txt" } else { "old.txt" };
            let mut changes = vec![json!({"path":path,"op":"put","content":STANDARD.encode(text)})];
            if renamed {
                changes.push(json!({"path":"old.txt","op":"delete"}));
            }
            let result =
                checked(&admin, &format!("{root}/workspaces/{name}/changes"), json!({"changes":changes})).await;
            heads.push(result["head"].as_str().unwrap().to_owned());
        }
        checked(&admin, &format!("{root}/proposals"), json!({"stream":"main","head":heads[1],"title":"stream edit"}))
            .await;
        checked(
            &admin,
            &format!("{root}/streams/cas"),
            json!({"stream":"main","expected_version":1,"expected_head":seed.to_string(),"new_head":heads[1]}),
        )
        .await;
        let result =
            checked(&admin, &format!("{root}/workspaces/ours/sync"), json!({"expected_stream_head":heads[1]})).await;
        let ours_path = if workspace_renames { "new.txt" } else { "old.txt" };
        let theirs_path = if workspace_renames { "old.txt" } else { "new.txt" };
        assert_eq!(result["status"], "conflicted");
        assert_eq!(result["conflicts"][0]["path"], "new.txt");
        assert_eq!(result["conflicts"][0]["origin"], json!({"base":"old.txt","ours":ours_path,"theirs":theirs_path}));
        assert_eq!(admin.get(&format!("{root}/workspaces/ours")).await.json["head"], heads[0]);
        let mut grpc = WorkspaceServiceClient::connect(server.base.clone()).await.unwrap();
        let mut request = Request::new(v1::SyncWorkspaceRequest {
            repo: repo.clone(),
            name: "ours".into(),
            expected_stream_head: heads[1].clone(),
            ..Default::default()
        });
        request.metadata_mut().insert("authorization", format!("Bearer {}", admin.token).parse().unwrap());
        let response = grpc.sync(request).await.unwrap().into_inner();
        assert_eq!(serde_json::to_value(response).unwrap()["conflicts"][0]["origin"], result["conflicts"][0]["origin"]);
        let proposal = checked(
            &admin,
            &format!("{root}/proposals"),
            json!({"stream":"main","head":heads[0],"title":"rename conflict"}),
        )
        .await;
        let prepared =
            checked(&admin, &format!("{root}/proposals/{}/prepare", proposal["id"].as_str().unwrap()), json!({})).await;
        assert_eq!(prepared["conflicts"][0]["origin"], json!({"base":"old.txt","ours":theirs_path,"theirs":ours_path}));
    }
    server.stop().await;
}
