//! A review draft belongs only to the current caller, never to the whole proposal.
mod common;
use ag_protocol::v1::{self, proposal_service_client::ProposalServiceClient};
use common::*;
use serde_json::json;
use tonic::{Code, Request};
fn authorized<T>(token: &str, value: T) -> Request<T> {
    let mut request = Request::new(value);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_drafts_are_private_and_versioned_without_submitting_reviews() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/drafts"})).await.status, 200);
    let bob = user_with_role(&srv, "user:bob", "acme/drafts", "reviewer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/drafts", "reviewer").await;
    let (head, objects) =
        Client::new("acme/drafts").commit(vec![], "user:admin", "change", &[("file", b"content")], 1_700_000_000_000);
    upload(&admin, "acme/drafts", &objects).await;
    let proposal = admin
        .post("/v1/repos/acme/drafts/proposals", json!({"stream":"main","head":head.to_string(),"title":"review"}))
        .await;
    let id = proposal.json["id"].as_str().unwrap();
    let path = format!("/v1/repos/acme/drafts/proposals/{id}/review-draft");
    let empty = bob.get(&path).await;
    assert_eq!(empty.status, 200, "{}", empty.json);
    assert_eq!(empty.headers.get("cache-control").map(String::as_str), Some("no-store"));
    assert_eq!(empty.json["present"], false);
    assert_eq!(empty.json["version"], 0);
    let scoped =
        bob.post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["acme/drafts"],"streams":["dev"]}})).await;
    assert_eq!(scoped.status, 200);
    let narrow = srv.client(scoped.json["token"].as_str().unwrap());
    assert_eq!(narrow.get(&path).await.status, 403);
    let reader = user_with_role(&srv, "user:reader", "acme/drafts", "reader").await;
    assert_eq!(reader.get(&path).await.status, 403);
    let body = json!({"head":head.to_string(),"decision":"comment","body":"Bob's private notes","expected_version":0});
    let saved = bob.call("PUT", &path, Some(body.clone()), &[]).await;
    assert_eq!(saved.status, 200, "{}", saved.json);
    assert_eq!(saved.json["version"], 1);
    assert_eq!(bob.call("PUT", &path, Some(body), &[]).await.json["version"], 1);
    let mut grpc = ProposalServiceClient::connect(srv.base.clone()).await.unwrap();
    let get = v1::GetReviewDraftRequest { repo: "acme/drafts".into(), id: id.into() };
    assert_eq!(
        grpc.get_draft(authorized(&bob.token, get.clone())).await.unwrap().into_inner().body,
        "Bob's private notes"
    );
    assert!(!grpc.get_draft(authorized(&carol.token, get)).await.unwrap().into_inner().present);
    let stale = v1::SaveReviewDraftRequest {
        repo: "acme/drafts".into(),
        id: id.into(),
        head: head.to_string(),
        decision: "comment".into(),
        body: "stale".into(),
        expected_version: 0,
    };
    assert_eq!(grpc.save_draft(authorized(&bob.token, stale)).await.unwrap_err().code(), Code::FailedPrecondition);
    for other in [&carol, &admin] {
        let private = other.get(&format!("{path}?owner=user:bob")).await;
        assert_eq!(private.status, 200);
        assert_eq!(private.json["present"], false);
        assert_ne!(private.json["body"], "Bob's private notes");
    }
    assert_eq!(
        bob.call(
            "PUT",
            &path,
            Some(json!({"head":head.to_string(),"decision":"approve","body":"stale","expected_version":0})),
            &[]
        )
        .await
        .status,
        412
    );
    let cleared = bob.call("DELETE", &format!("{path}?expected_version=1"), None, &[]).await;
    assert_eq!(cleared.status, 200, "{}", cleared.json);
    assert_eq!(cleared.json["version"], 2);
    assert_eq!(cleared.json["present"], false);
    assert_eq!(cleared.json["body"], "");
    assert_eq!(
        bob.call(
            "PUT",
            &path,
            Some(json!({"head":head.to_string(),"decision":"comment","body":"old device","expected_version":1})),
            &[]
        )
        .await
        .status,
        412
    );
    let clear = v1::ClearReviewDraftRequest { repo: "acme/drafts".into(), id: id.into(), expected_version: 1 };
    assert_eq!(grpc.clear_draft(authorized(&bob.token, clear)).await.unwrap().into_inner().version, 2);
    let detail = admin.get(&format!("/v1/repos/acme/drafts/proposals/{id}")).await;
    assert!(detail.json["reviews"].as_array().unwrap().is_empty());
    let audit = admin.get(&format!("/v1/repos/acme/drafts/audit?contains={id}")).await;
    assert!(!audit.json.to_string().contains("Bob's private notes"));
    srv.stop().await;
}
