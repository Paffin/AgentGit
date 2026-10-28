//! Real HTTP search: immutable pagination, exclusions, input and repository authorization.
mod common;

use common::*;
use serde_json::json;

async fn seed(srv: &TestServer, files: &[(&str, &[u8])]) -> (Http, String) {
    assert_eq!(srv.admin().post("/v1/repos", json!({"name":"acme/search"})).await.status, 200);
    let writer = user_with_role(srv, "user:writer", "acme/search", "writer").await;
    let (head, mut objects) =
        Client::new("acme/search").commit(vec![], "user:writer", "Search data", files, 1_700_000_000_000);
    let mut seen = std::collections::HashSet::new();
    objects.retain(|object| seen.insert(object.id()));
    upload(&writer, "acme/search", &objects).await;
    let proposal = writer
        .post("/v1/repos/acme/search/proposals", json!({"stream":"main","head":head.to_string(),"title":"Search data"}))
        .await;
    assert_eq!(proposal.status, 200, "{}", proposal.json);
    let landed = srv
        .admin()
        .post(
            &format!("/v1/repos/acme/search/proposals/{}/integrate", proposal.json["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(landed.json["outcome"]["status"], "integrated", "{}", landed.json);
    (writer, head.to_string())
}

#[tokio::test]
async fn search_pages_preserve_matches_and_report_skipped_files() {
    let srv = TestServer::start().await;
    let huge = vec![b'x'; 1024 * 1024 + 1];
    let (_, head) = seed(
        &srv,
        &[
            ("docs/start.md", b"Needle\n"),
            ("src/a.rs", b"Needle\nneedle\n"),
            ("data.bin", b"Needle\0binary"),
            ("huge.txt", &huge),
        ],
    )
    .await;
    let reader = user_with_role(&srv, "user:reader", "acme/search", "reader").await;
    let mut token = String::new();
    let mut found = Vec::new();
    let mut skipped = (0, 0);
    for _ in 0..10 {
        let response = reader
            .get(&format!("/v1/repos/acme/search/search?rev={head}&query=needle&page_size=1&page_token={token}"))
            .await;
        assert_eq!(response.status, 200, "{}", response.json);
        assert_eq!(response.json["rev"], head);
        for item in response.json["matches"].as_array().unwrap() {
            found.push((item["path"].as_str().unwrap().to_owned(), item["line"].as_u64().unwrap()));
        }
        skipped.0 += response.json["skipped_binary"].as_u64().unwrap();
        skipped.1 += response.json["skipped_large"].as_u64().unwrap();
        token = response.json["next_page_token"].as_str().unwrap().to_owned();
        if token.is_empty() {
            break;
        }
    }
    assert!(token.is_empty());
    assert_eq!(found, vec![("docs/start.md".into(), 1), ("src/a.rs".into(), 1), ("src/a.rs".into(), 2)]);
    assert_eq!(skipped, (1, 1));
    let exact = reader.get("/v1/repos/acme/search/search?query=needle&case_sensitive=true&path_prefix=src").await;
    assert_eq!(exact.status, 200);
    assert_eq!(exact.json["matches"].as_array().unwrap().len(), 1);
    assert_eq!(exact.json["matches"][0]["line"], 2);
    let channel = tonic::transport::Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap();
    let mut request = tonic::Request::new(ag_protocol::v1::SearchTreeRequest {
        repo: "acme/search".into(),
        rev: head,
        query: "needle".into(),
        case_sensitive: true,
        path_prefix: "src".into(),
        ..Default::default()
    });
    request.metadata_mut().insert("authorization", format!("Bearer {}", reader.token).parse().unwrap());
    let grpc = ag_protocol::v1::tree_service_client::TreeServiceClient::new(channel)
        .search(request)
        .await
        .unwrap()
        .into_inner();
    assert_eq!(grpc.matches.len(), 1);
    assert_eq!((grpc.matches[0].path.as_str(), grpc.matches[0].line), ("src/a.rs", 2));
    srv.stop().await;
}

#[tokio::test]
async fn search_rejects_bad_inputs_changed_cursors_and_unrelated_readers() {
    let srv = TestServer::start().await;
    let (writer, head) = seed(&srv, &[("a.txt", b"one\none\n")]).await;
    let reader = user_with_role(&srv, "user:reader", "acme/search", "reader").await;
    let unrelated = user_with_role(&srv, "user:other", "elsewhere/*", "reader").await;
    // Unrelated repositories are hidden, not exposed through an authorization oracle.
    assert_eq!(unrelated.get("/v1/repos/acme/search/search?query=one").await.status, 404);
    assert_eq!(srv.client("").get("/v1/repos/acme/search/search?query=one").await.status, 401);
    for query in ["query=", "query=%0A", "query=one&path_prefix=../secret", "query=one&page_token=garbage"] {
        assert_eq!(reader.get(&format!("/v1/repos/acme/search/search?{query}")).await.status, 400, "{query}");
    }
    let first = reader.get(&format!("/v1/repos/acme/search/search?rev={head}&query=one&page_size=1")).await;
    let token = first.json["next_page_token"].as_str().unwrap();
    assert!(!token.is_empty());
    assert_eq!(
        reader.get(&format!("/v1/repos/acme/search/search?rev={head}&query=two&page_token={token}")).await.status,
        400
    );
    assert_eq!(
        reader
            .get(&format!("/v1/repos/acme/search/search?rev={head}&query=one&case_sensitive=true&page_token={token}"))
            .await
            .status,
        400
    );
    let (new_head, objects) = Client::new("acme/search").commit(
        vec![head.parse().unwrap()],
        "user:writer",
        "Advance branch",
        &[("a.txt", b"one changed\n")],
        1_700_000_000_001,
    );
    upload(&writer, "acme/search", &objects).await;
    let proposal = writer
        .post(
            "/v1/repos/acme/search/proposals",
            json!({"stream":"main","head":new_head.to_string(),"title":"Advance branch"}),
        )
        .await;
    let landed = srv
        .admin()
        .post(
            &format!("/v1/repos/acme/search/proposals/{}/integrate", proposal.json["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(landed.json["outcome"]["status"], "integrated");
    assert_eq!(
        reader.get(&format!("/v1/repos/acme/search/search?rev=main&query=one&page_token={token}")).await.status,
        400
    );
    let pinned = reader.get(&format!("/v1/repos/acme/search/search?rev={head}&query=one&page_token={token}")).await;
    assert_eq!(pinned.status, 200);
    assert_eq!(pinned.json["matches"][0]["line"], 2);
    srv.stop().await;
}

#[tokio::test]
async fn empty_match_pages_can_resume_the_bounded_scan() {
    let srv = TestServer::start().await;
    let names: Vec<_> = (0..1001).map(|i| format!("file{i:04}.txt")).collect();
    let files: Vec<_> = names.iter().map(|name| (name.as_str(), b"empty\n".as_slice())).collect();
    let (writer, head) = seed(&srv, &files).await;
    let first = writer.get(&format!("/v1/repos/acme/search/search?rev={head}&query=missing")).await;
    assert_eq!(first.status, 200, "{}", first.json);
    assert_eq!(first.json["files_scanned"], 1000);
    assert!(first.json["matches"].as_array().unwrap().is_empty());
    let token = first.json["next_page_token"].as_str().unwrap();
    assert!(!token.is_empty());
    let last = writer.get(&format!("/v1/repos/acme/search/search?rev={head}&query=missing&page_token={token}")).await;
    assert_eq!(last.status, 200, "{}", last.json);
    assert_eq!(last.json["files_scanned"], 1);
    assert_eq!(last.json["next_page_token"], "");
    srv.stop().await;
}
