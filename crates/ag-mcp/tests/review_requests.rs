//! Requested review routing through the real stdio bridge.
#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::json;

#[test]
fn authors_manage_review_requests_without_granting_access_or_approval() {
    let srv = TestServer::start();
    let repo = "acme/reviews";
    srv.repo(repo);
    srv.seed(repo, "Initial", &[("file.txt", "old\n")]);
    let author = srv.credential("user:alice", repo, "writer", 3600, &[]);
    let reader = srv.credential("user:bob", repo, "reader", 3600, &[]);
    let mut m = Mcp::start(&srv, &author, &["--repo", repo]);
    m.ok("workspace_create", json!({"name":"change"}));
    m.ok(
        "workspace_write_files",
        json!({"workspace":"change","message":"change","changes":[{"path":"file.txt","op":"put","content":"new\n"}]}),
    );
    let proposal = m.ok("propose", json!({"workspace":"change","title":"Review this"}));
    let id = proposal.structured["proposal"]["id"].as_str().unwrap();
    let set = json!({"proposal_id":id,"reviewers":["user:bob"],"expected_reviewers":[]});
    let saved = m.ok("proposal_set_reviewers", set.clone());
    assert_eq!(saved.structured["reviewers"], json!(["user:bob"]));
    assert_eq!(m.ok("proposal_set_reviewers", set).structured["reviewers"], json!(["user:bob"]));
    let missing_guard = m.call("proposal_set_reviewers", json!({"proposal_id":id,"reviewers":[]}));
    assert!(missing_guard.is_error, "the inspected set is required before a write");
    let status = m.ok("proposal_status", json!({"proposal_id":id}));
    assert_eq!(status.structured["requested_reviewers"], json!(["user:bob"]));
    assert!(status.structured["reviews"]["approvals"].as_array().unwrap().is_empty());
    let listed = m.ok("proposal_list", json!({"requested_reviewer":"user:bob"}));
    assert_eq!(listed.structured["proposals"][0]["id"], id);
    let absent = m.ok("proposal_list", json!({"requested_reviewer":"user:carol"}));
    assert!(absent.structured["proposals"].as_array().unwrap().is_empty());
    let stale = m.call("proposal_set_reviewers", json!({"proposal_id":id,"reviewers":[],"expected_reviewers":[]}));
    assert!(stale.is_error, "stale set must not erase a newer request");
    let mut other = Mcp::start(&srv, &reader, &["--repo", repo]);
    assert!(
        other
            .call("proposal_set_reviewers", json!({"proposal_id":id,"reviewers":[],"expected_reviewers":["user:bob"]}))
            .is_error
    );
    let mut read_only = Mcp::start(&srv, &reader, &["--repo", repo, "--allow", "read"]);
    assert!(!read_only.tool_names().contains(&"proposal_set_reviewers".into()));
    m.ok("proposal_set_reviewers", json!({"proposal_id":id,"reviewers":[],"expected_reviewers":["user:bob"]}));
    assert!(
        m.ok("proposal_status", json!({"proposal_id":id})).structured["requested_reviewers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn older_servers_refuse_review_request_writes_and_filters_before_using_them() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut input = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        assert!(line.starts_with("GET /v1/capabilities "), "{line}");
        loop {
            let mut header = String::new();
            input.read_line(&mut header).unwrap();
            if header == "\r\n" || header.is_empty() {
                break;
            }
        }
        let body = r#"{"features":[]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let mut m = Mcp::spawn(
        &["--server", &url, "--repo", "acme/reviews", "--allow", "read,propose,review"],
        &[("AG_MCP_TOKEN", "test-token")],
    );
    m.request("server/discover", json!({}));
    m.refused(
        "proposal_set_reviewers",
        json!({"proposal_id":"prop_test","reviewers":[],"expected_reviewers":[]}),
        "server_update_required",
    );
    m.refused("proposal_list", json!({"requested_reviewer":"user:bob"}), "server_update_required");
    m.refused("review_draft_get", json!({"proposal_id":"prop_test"}), "server_update_required");
    m.refused("review_draft_save", json!({"proposal_id":"prop_test","expected_version":0,"head":"0".repeat(64),"decision":"comment","body":"private"}), "server_update_required");
    m.refused("review_draft_clear", json!({"proposal_id":"prop_test","expected_version":0}), "server_update_required");
    m.refused("comment_draft_get", json!({"proposal_id":"prop_test"}), "server_update_required");
    m.refused(
        "comment_draft_save",
        json!({"proposal_id":"prop_test","expected_version":0,"head":"0".repeat(64),"body":"private"}),
        "server_update_required",
    );
    m.refused("comment_draft_clear", json!({"proposal_id":"prop_test","expected_version":0}), "server_update_required");
    response.join().unwrap();
}
