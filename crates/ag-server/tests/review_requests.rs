//! Review routing respects author permissions and optimistic edits.
mod common;
use ag_protocol::v1::{self, proposal_service_client::ProposalServiceClient};
use common::*;
use serde_json::json;
use tonic::{Code, Request};
fn authed<T>(token: &str, value: T) -> Request<T> {
    let mut request = Request::new(value);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requested_reviewers_are_visible_but_only_the_author_can_change_them() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/reviews"})).await.status, 200);
    let author = user_with_role(&srv, "user:author", "acme/reviews", "writer").await;
    let reviewer = user_with_role(&srv, "user:bob", "acme/reviews", "reviewer").await;
    let (head, objects) =
        Client::new("acme/reviews").commit(vec![], "user:author", "change", &[("file", b"content")], 1_700_000_000_000);
    upload(&author, "acme/reviews", &objects).await;
    let proposal = author
        .post("/v1/repos/acme/reviews/proposals", json!({"stream":"main","head":head.to_string(),"title":"review"}))
        .await;
    assert_eq!(proposal.status, 200);
    let id = proposal.json["id"].as_str().unwrap();
    let path = format!("/v1/repos/acme/reviews/proposals/{id}/reviewers");
    let body = json!({"reviewers":["user:bob"],"expected_reviewers":[]});
    let saved = author.call("PUT", &path, Some(body.clone()), &[]).await;
    assert_eq!(saved.status, 200, "{}", saved.json);
    assert_eq!(saved.json["reviewers"], json!(["user:bob"]));
    assert_eq!(author.call("PUT", &path, Some(body), &[]).await.status, 200);
    let detail = reviewer.get(&format!("/v1/repos/acme/reviews/proposals/{id}")).await;
    assert_eq!(detail.json["requested_reviewers"], json!(["user:bob"]));
    assert!(detail.json["reviews"].as_array().unwrap().is_empty());
    let matches = reviewer.get("/v1/repos/acme/reviews/proposals?requested_reviewer=user:bob").await;
    assert_eq!(matches.json["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(
        reviewer.get("/v1/repos/acme/reviews/proposals?requested_reviewer=user:carol").await.json["proposals"],
        json!([])
    );
    let scoped = author
        .post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["acme/reviews"],"streams":["dev"]}}))
        .await;
    assert_eq!(scoped.status, 200);
    let narrow = srv.client(scoped.json["token"].as_str().unwrap());
    assert_eq!(
        narrow.call("PUT", &path, Some(json!({"reviewers":[],"expected_reviewers":["user:bob"]})), &[]).await.status,
        403
    );
    let clear = json!({"reviewers":[],"expected_reviewers":["user:bob"]});
    assert_eq!(reviewer.call("PUT", &path, Some(clear.clone()), &[]).await.status, 403);
    assert_eq!(author.call("PUT", &path, Some(json!({"reviewers":[],"expected_reviewers":[]})), &[]).await.status, 412);
    assert_eq!(author.call("PUT", &path, Some(clear), &[]).await.status, 200);
    let mut grpc = ProposalServiceClient::connect(srv.base.clone()).await.unwrap();
    let request = v1::SetRequestedReviewersRequest {
        repo: "acme/reviews".into(),
        id: id.into(),
        reviewers: vec!["user:bob".into()],
        expected_reviewers: vec![],
    };
    assert_eq!(
        grpc.set_reviewers(authed(&author.token, request.clone())).await.unwrap().into_inner().reviewers,
        ["user:bob"]
    );
    assert_eq!(grpc.set_reviewers(authed(&reviewer.token, request)).await.unwrap_err().code(), Code::PermissionDenied);
    let stale = v1::SetRequestedReviewersRequest {
        repo: "acme/reviews".into(),
        id: id.into(),
        reviewers: vec![],
        expected_reviewers: vec![],
    };
    assert_eq!(grpc.set_reviewers(authed(&author.token, stale)).await.unwrap_err().code(), Code::FailedPrecondition);
    let principal_request = json!({"reviewers":["agent:reviewer#run-a"],"expected_reviewers":["user:bob"]});
    assert_eq!(author.call("PUT", &path, Some(principal_request), &[]).await.status, 200);
    let matches = reviewer.get("/v1/repos/acme/reviews/proposals?requested_reviewer=agent:reviewer%23run-a").await;
    assert_eq!(matches.json["proposals"].as_array().unwrap().len(), 1);
    srv.stop().await;
}
