//! Stream-scoped credentials may review only proposals targeting an allowed stream.
mod common;
use ag_protocol::v1::{self, proposal_service_client::ProposalServiceClient};
use common::*;
use serde_json::json;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

fn authed<T>(token: &str, value: T) -> Request<T> {
    let mut request = Request::new(value);
    request.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {token}")).unwrap());
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviews_enforce_stream_scope_over_http_and_grpc() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/scoped"})).await.status, 200);
    assert_eq!(admin.post("/v1/repos/acme/scoped/streams", json!({"name":"dev"})).await.status, 200);
    let author = user_with_role(&srv, "user:author", "acme/scoped", "writer").await;
    let reviewer = user_with_role(&srv, "user:reviewer", "acme/scoped", "reviewer").await;
    let minted = reviewer
        .post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["acme/scoped"],"streams":["dev"]}}))
        .await;
    assert_eq!(minted.status, 200, "{}", minted.json);
    let token = minted.json["token"].as_str().unwrap();
    let narrow = srv.client(token);
    let (head, objects) = Client::new("acme/scoped").commit(
        vec![],
        "user:author",
        "change",
        &[("file.txt", b"content")],
        1_700_000_000_000,
    );
    upload(&author, "acme/scoped", &objects).await;
    let main = author
        .post("/v1/repos/acme/scoped/proposals", json!({"stream":"main","head":head.to_string(),"title":"main change"}))
        .await;
    let dev = author
        .post("/v1/repos/acme/scoped/proposals", json!({"stream":"dev","head":head.to_string(),"title":"dev change"}))
        .await;
    assert_eq!(main.status, 200, "{}", main.json);
    assert_eq!(dev.status, 200, "{}", dev.json);
    let main_id = main.json["id"].as_str().unwrap();
    let dev_id = dev.json["id"].as_str().unwrap();
    assert_eq!(narrow.get("/v1/repos/acme/scoped/streams/head?stream=main").await.status, 403);
    let channel = Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap();
    let mut grpc = ProposalServiceClient::new(channel);
    for decision in ["approve", "request_changes"] {
        let denied = narrow
            .post(
                &format!("/v1/repos/acme/scoped/proposals/{main_id}/reviews"),
                json!({"decision":decision,"expected_head":head.to_string()}),
            )
            .await;
        assert_eq!(denied.status, 403, "{}", denied.json);
        let request = v1::ReviewProposalRequest {
            repo: "acme/scoped".into(),
            id: main_id.into(),
            decision: decision.into(),
            expected_head: head.to_string(),
            ..Default::default()
        };
        assert_eq!(grpc.review(authed(token, request)).await.unwrap_err().code(), Code::PermissionDenied);
        let allowed = narrow
            .post(
                &format!("/v1/repos/acme/scoped/proposals/{dev_id}/reviews"),
                json!({"decision":decision,"expected_head":head.to_string()}),
            )
            .await;
        assert_eq!(allowed.status, 200, "{}", allowed.json);
        let request = v1::ReviewProposalRequest {
            repo: "acme/scoped".into(),
            id: dev_id.into(),
            decision: decision.into(),
            expected_head: head.to_string(),
            ..Default::default()
        };
        assert!(grpc.review(authed(token, request)).await.is_ok());
    }
    let author_token = author
        .post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["acme/scoped"],"streams":["dev"]}}))
        .await;
    assert_eq!(author_token.status, 200);
    let narrow_author = srv.client(author_token.json["token"].as_str().unwrap());
    let main_path = format!("/v1/repos/acme/scoped/proposals/{main_id}");
    assert_eq!(
        narrow_author
            .call("PATCH", &main_path, Some(json!({"new_head":head.to_string(),"expected_head":head.to_string()})), &[])
            .await
            .status,
        403
    );
    assert_eq!(
        narrow_author
            .post(&format!("{main_path}/comments"), json!({"body":"outside scope", "expected_head":head.to_string()}))
            .await
            .status,
        403
    );
    assert_eq!(narrow_author.post(&format!("{main_path}/close"), json!({})).await.status, 403);
    let main_after = admin.get(&format!("/v1/repos/acme/scoped/proposals/{main_id}")).await;
    assert!(main_after.json["reviews"].as_array().unwrap().is_empty());
    let audit = admin.get(&format!("/v1/repos/acme/scoped/audit?contains={main_id}")).await;
    assert!(!audit.json["entries"].as_array().unwrap().iter().any(|entry| {
        ["proposal.review", "proposal.comment", "proposal.update", "proposal.close"]
            .iter()
            .any(|action| entry["action"] == *action)
    }));
    srv.stop().await;
}
