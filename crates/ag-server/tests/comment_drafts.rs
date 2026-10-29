//! Account discussion notes are private, versioned and independent of review decisions.
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
async fn writers_can_resume_private_comment_drafts_without_posting_reviews() {
    let server = TestServer::start().await;
    let admin = server.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name":"acme/comments"})).await.status, 200);
    let writer = user_with_role(&server, "user:writer", "acme/comments", "writer").await;
    let other = user_with_role(&server, "user:other", "acme/comments", "reviewer").await;
    let reader = user_with_role(&server, "user:reader", "acme/comments", "reader").await;
    let (head, objects) =
        Client::new("acme/comments").commit(vec![], "user:admin", "change", &[("file", b"content")], 1_700_000_000_000);
    upload(&admin, "acme/comments", &objects).await;
    let proposal = admin
        .post("/v1/repos/acme/comments/proposals", json!({"stream":"main","head":head.to_string(),"title":"comment"}))
        .await;
    let id = proposal.json["id"].as_str().unwrap();
    let path = format!("/v1/repos/acme/comments/proposals/{id}/comment-draft");
    let absent = writer.get(&path).await;
    assert_eq!(absent.status, 200, "{}", absent.json);
    assert_eq!(absent.json["present"], false);
    assert_eq!(absent.json["version"], 0);
    assert_eq!(absent.headers.get("cache-control").map(String::as_str), Some("no-store"));
    let body = json!({"head":head.to_string(),"body":"Writer's private note","expected_version":0});
    let saved = writer.call("PUT", &path, Some(body.clone()), &[]).await;
    assert_eq!(saved.status, 200, "{}", saved.json);
    assert_eq!(saved.json["version"], 1);
    assert_eq!(writer.call("PUT", &path, Some(body), &[]).await.json["version"], 1);
    let mut grpc = ProposalServiceClient::connect(server.base.clone()).await.unwrap();
    let get = v1::GetCommentDraftRequest { repo: "acme/comments".into(), id: id.into() };
    assert_eq!(
        grpc.get_comment_draft(authorized(&writer.token, get.clone())).await.unwrap().into_inner().body,
        "Writer's private note"
    );
    assert!(!grpc.get_comment_draft(authorized(&other.token, get)).await.unwrap().into_inner().present);
    let stale = v1::SaveCommentDraftRequest {
        repo: "acme/comments".into(),
        id: id.into(),
        head: head.to_string(),
        body: "stale".into(),
        expected_version: 0,
    };
    assert_eq!(
        grpc.save_comment_draft(authorized(&writer.token, stale)).await.unwrap_err().code(),
        Code::FailedPrecondition
    );
    let invalid = json!({"head":head.to_string(),"body":"é".repeat(8193),"expected_version":1});
    assert_eq!(writer.call("PUT", &path, Some(invalid), &[]).await.status, 400);
    for account in [&other, &admin] {
        assert_eq!(account.get(&format!("{path}?owner=user:writer")).await.json["present"], false);
    }
    let denied = reader.get(&path).await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.headers.get("cache-control").map(String::as_str), Some("no-store"));
    let narrow = writer
        .post("/v1/credentials", json!({"ttl_secs":600,"scope":{"repos":["acme/comments"],"streams":["other"]}}))
        .await;
    assert_eq!(narrow.status, 200, "{}", narrow.json);
    assert_eq!(server.client(narrow.json["token"].as_str().unwrap()).get(&path).await.status, 403);
    assert_eq!(
        writer
            .call("PUT", &path, Some(json!({"head":head.to_string(),"body":"stale","expected_version":0})), &[])
            .await
            .status,
        412
    );

    let review_path = format!("/v1/repos/acme/comments/proposals/{id}/review-draft");
    assert_eq!(admin.call("PUT", &review_path, Some(json!({"head":head.to_string(),"decision":"approve","body":"independent review","expected_version":0})), &[]).await.status,200);
    assert_eq!(
        admin
            .call(
                "PUT",
                &path,
                Some(json!({"head":head.to_string(),"body":"independent discussion","expected_version":0})),
                &[]
            )
            .await
            .status,
        200
    );
    assert_eq!(admin.call("DELETE", &format!("{path}?expected_version=1"), None, &[]).await.status, 200);
    assert_eq!(admin.get(&review_path).await.json["body"], "independent review");
    assert_eq!(admin.post(&format!("/v1/repos/acme/comments/proposals/{id}/close"), json!({})).await.status, 200);
    let closed = writer
        .call(
            "PUT",
            &path,
            Some(json!({"head":head.to_string(),"body":"still private after closure","expected_version":1})),
            &[],
        )
        .await;
    assert_eq!(closed.status, 200, "{}", closed.json);
    assert_eq!(closed.json["version"], 2);
    let cleared = writer.call("DELETE", &format!("{path}?expected_version=2"), None, &[]).await;
    assert_eq!(cleared.status, 200, "{}", cleared.json);
    assert_eq!(cleared.json["version"], 3);
    assert_eq!(cleared.json["present"], false);
    assert_eq!(cleared.json["body"], "");
    assert_eq!(writer.call("DELETE", &format!("{path}?expected_version=2"), None, &[]).await.json["version"], 3);
    assert_eq!(
        writer
            .call("PUT", &path, Some(json!({"head":head.to_string(),"body":"late","expected_version":2})), &[])
            .await
            .status,
        412
    );
    let clear = v1::ClearCommentDraftRequest { repo: "acme/comments".into(), id: id.into(), expected_version: 2 };
    assert_eq!(grpc.clear_comment_draft(authorized(&writer.token, clear)).await.unwrap().into_inner().version, 3);
    let detail = admin.get(&format!("/v1/repos/acme/comments/proposals/{id}")).await;
    assert!(detail.json["reviews"].as_array().unwrap().is_empty());
    let audit = admin.get(&format!("/v1/repos/acme/comments/audit?contains={id}")).await;
    assert!(!audit.json.to_string().contains("Writer's private note"));
    server.stop().await;
}
