//! Pinned conflict resolutions preserve their source and reject an obsolete target.
#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::json;

#[test]
fn workspace_sync_returns_and_checks_the_inspected_stream_head() {
    let srv = TestServer::start();
    let repo = "acme/pinned";
    srv.repo(repo);
    srv.seed(repo, "base", &[("file", "base\n")]);
    let author = srv.credential("user:author", repo, "writer", 3600, &[]);
    let mut m = Mcp::start(&srv, &author, &["--repo", repo]);
    m.ok("workspace_create", json!({"name":"mine"}));
    let written = m.ok(
        "workspace_write_files",
        json!({"workspace":"mine","message":"ours","changes":[{"path":"file","op":"put","content":"ours\n"}]}),
    );
    let source = srv
        .http(&author)
        .get(&format!("/v1/repos/{repo}/tree/resolve?path=file&rev={}", written.structured["head"].as_str().unwrap()));
    assert_eq!(source.0, 200);
    let blob = source.1["entry"]["blob_id"].as_str().unwrap();
    let inspected = srv.seed(repo, "theirs", &[("file", "theirs\n")]);
    let sync = m.ok("workspace_sync", json!({"workspace":"mine"}));
    assert_eq!(sync.structured["status"], "conflicted");
    assert_eq!(sync.structured["stream_head"], inspected);
    let newer = srv.seed(repo, "newer", &[("file", "newer\n")]);
    m.refused("workspace_sync", json!({"workspace":"mine","expected_stream_head":inspected,"resolutions":[{"path":"file","content":"resolved\n"}]}), "failed_precondition");
    let done = m.ok("workspace_sync", json!({"workspace":"mine","expected_stream_head":newer,"resolutions":[{"path":"file","blob_id":blob,"file_type":"regular","executable":true}]}));
    assert_eq!(done.structured["status"], "merged");
    assert_eq!(done.structured["stream_head"], newer);
    let file = m.ok("read_file", json!({"path":"file","rev":"@mine"}));
    assert!(file.text.contains("ours"));
    let result = srv
        .http(&author)
        .get(&format!("/v1/repos/{repo}/tree/resolve?path=file&rev={}", done.structured["head"].as_str().unwrap()));
    assert_eq!(result.0, 200);
    assert_eq!(result.1["entry"]["blob_id"], blob);
    assert_eq!(result.1["entry"]["executable"], true);
}

#[test]
fn older_servers_refuse_pinned_sync_without_writing() {
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
    let mut m = Mcp::spawn(&["--server", &url, "--repo", "acme/pinned"], &[("AG_MCP_TOKEN", "test")]);
    m.request("server/discover", json!({}));
    m.refused("workspace_sync", json!({"workspace":"mine","expected_stream_head":"seen"}), "server_update_required");
    response.join().unwrap();
}
