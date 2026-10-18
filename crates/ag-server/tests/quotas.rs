//! Repository quotas through a real server: every way bytes enter a repository is counted, the per-object limit
//! applies everywhere, defaults come from the configuration, and nothing is buffered for a caller who is not
//! allowed to write.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use ag_protocol::v1;
use ag_protocol::v1::object_service_client::ObjectServiceClient;
use common::*;
use serde_json::{Value, json};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Bytes the server counts for `repo`.
fn tracked(srv: &TestServer, repo: &str) -> u64 {
    srv.app.control.repo(repo).unwrap().unwrap().object_bytes
}

async fn set_quota(srv: &TestServer, repo: &str, quota: Value) {
    let r = srv.admin().call("PATCH", &format!("/v1/repos/{repo}"), Some(json!({"quota": quota})), &[]).await;
    assert_eq!(r.status, 200, "{}", r.json);
}

async fn workspace(http: &Http, repo: &str, name: &str) {
    let r = http.post(&format!("/v1/repos/{repo}/workspaces"), json!({"name": name, "stream": "main"})).await;
    assert_eq!(r.status, 200, "{}", r.json);
}

async fn put_file(http: &Http, repo: &str, ws: &str, path: &str, content: &[u8]) -> Reply {
    http.post(
        &format!("/v1/repos/{repo}/workspaces/{ws}/changes"),
        json!({"changes": [{"path": path, "op": "put", "content": b64(content)}], "message": "edit"}),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_edits_are_counted_and_limited_like_uploads() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    // The uploads of the existing quota test are not the only way in: a virtual workspace writes blobs and
    // changesets too (inline content, up to the request limit per call, any number of calls).
    set_quota(&srv, "acme/web", json!({"max_total_object_bytes": 6000})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    workspace(&alice, "acme/web", "w1").await;

    let before = tracked(&srv, "acme/web");
    let r = put_file(&alice, "acme/web", "w1", "a.txt", &vec![b'a'; 2000]).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let after_first = tracked(&srv, "acme/web");
    assert!(after_first >= before + 2000, "a workspace write is accounted ({before} -> {after_first})");
    let r = put_file(&alice, "acme/web", "w1", "b.txt", &vec![b'b'; 2000]).await;
    assert_eq!(r.status, 200, "{}", r.json);

    // The next one would pass the quota. Before the fix this succeeded, for ever.
    let r = put_file(&alice, "acme/web", "w1", "c.txt", &vec![b'c'; 4000]).await;
    assert_eq!(r.status, 429, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "quota_exceeded", "{}", r.json);
    assert!(
        tracked(&srv, "acme/web") <= 6000,
        "the tracked total never passes the quota: {}",
        tracked(&srv, "acme/web")
    );

    // Repeating the same content costs nothing (the object is already there) and is allowed.
    let again = put_file(&alice, "acme/web", "w1", "a-copy.txt", &vec![b'a'; 2000]).await;
    assert_eq!(again.status, 200, "{}", again.json);

    // Lifting the quota lets the write through; the count keeps growing.
    set_quota(&srv, "acme/web", json!({"max_total_object_bytes": 0})).await;
    let before = tracked(&srv, "acme/web");
    let r = put_file(&alice, "acme/web", "w1", "c.txt", &vec![b'c'; 4000]).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert!(tracked(&srv, "acme/web") >= before + 4000);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_an_integration_writes_is_counted_too() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let id = |v: &Value| v.as_str().unwrap().to_string();

    // A seed on main, then two workspaces editing different lines of the same file.
    let c = Client::new("acme/web");
    let (seed, objs) = c.commit(vec![], "user:alice", "seed", &[("f.txt", b"1\n2\n3\n4\n5\n")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objs).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": seed.to_string(), "title": "seed"}))
        .await;
    carol.post(&format!("/v1/repos/acme/web/proposals/{}/integrate", id(&p.json["id"])), json!({})).await;
    let mut proposals = Vec::new();
    for (ws, content) in [("w1", "ONE\n2\n3\n4\n5\n"), ("w2", "1\n2\n3\n4\nFIVE\n")] {
        workspace(&alice, "acme/web", ws).await;
        let r = put_file(&alice, "acme/web", ws, "f.txt", content.as_bytes()).await;
        let p = alice
            .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": id(&r.json["head"]), "title": ws}))
            .await;
        proposals.push(id(&p.json["id"]));
    }
    let integrate = |pid: String| {
        let carol = carol.clone();
        async move { carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await }
    };
    assert_eq!(integrate(proposals[0].clone()).await.json["outcome"]["strategy"], "fast_forward");

    // The second one has to be merged: a new blob, a new changeset. The engine writes them on its own account.
    let before = tracked(&srv, "acme/web");
    set_quota(&srv, "acme/web", json!({"max_total_object_bytes": before + 8})).await;
    let refused = integrate(proposals[1].clone()).await;
    assert_eq!(refused.status, 429, "{}", refused.json);
    assert_eq!(refused.json["error"]["code"], "quota_exceeded", "{}", refused.json);
    let main = alice.get("/v1/repos/acme/web/streams/head?stream=main").await;
    assert_eq!(main.json["version"], 2, "the stream did not move: {}", main.json);
    assert!(tracked(&srv, "acme/web") <= before + 8);

    // With room, the same integration goes through and the new objects are on the books.
    set_quota(&srv, "acme/web", json!({"max_total_object_bytes": 0})).await;
    let done = integrate(proposals[1].clone()).await;
    assert_eq!(done.json["outcome"]["strategy"], "merge", "{}", done.json);
    assert!(tracked(&srv, "acme/web") > before, "the merge wrote objects: {before} -> {}", tracked(&srv, "acme/web"));
    let f = alice.get("/v1/repos/acme/web/files/f.txt?rev=main").await;
    assert_eq!(f.body, b"ONE\n2\n3\n4\nFIVE\n");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_workspace_edits_cannot_pass_the_quota_between_them() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let quota = 30_000u64;
    set_quota(&srv, "acme/web", json!({"max_total_object_bytes": quota})).await;
    let mut tasks = Vec::new();
    for t in 0..6 {
        let alice = user_with_role(&srv, &format!("user:w{t}"), "acme/web", "writer").await;
        workspace(&alice, "acme/web", &format!("w{t}")).await;
        tasks.push(tokio::spawn(async move {
            let mut accepted = 0;
            for i in 0..8 {
                let content = vec![b'a' + t as u8; 3000 + i];
                let r = put_file(&alice, "acme/web", &format!("w{t}"), &format!("f{i}.bin"), &content).await;
                match r.status {
                    200 => accepted += 1,
                    429 => assert_eq!(r.json["error"]["code"], "quota_exceeded", "{}", r.json),
                    other => panic!("unexpected {other}: {}", r.json),
                }
            }
            accepted
        }));
    }
    let mut accepted = 0;
    for t in tasks {
        accepted += t.await.unwrap();
    }
    let total = tracked(&srv, "acme/web");
    assert!(
        total <= quota,
        "6 writers raced, {accepted} edits were accepted, and {total} bytes are tracked (quota {quota})"
    );
    assert!(accepted > 0 && accepted < 48, "some fit and some did not: {accepted}");
    // The count is what is really stored: a recount (what garbage collection does) agrees.
    let actual: u64 = srv.app.repos.handle("acme/web").unwrap().objects().list().unwrap().iter().map(|o| o.size).sum();
    assert!(actual <= quota, "{actual} bytes are on disk");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repository_can_limit_the_size_of_one_object_everywhere() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    set_quota(&srv, "acme/web", json!({"max_object_bytes": 1024})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let c = Client::new("acme/web");

    // HTTP upload: a small blob is fine, a 2 KiB one is too large - before it is fully read, and before the
    // server-wide limit (256 MiB) has anything to say.
    let (_, small) = c.commit(vec![], "user:alice", "s", &[("a.txt", &[b'x'; 100])], 1_700_000_000_000);
    upload(&alice, "acme/web", &small).await;
    let (_, big) = c.commit(vec![], "user:alice", "b", &[("b.bin", &[b'y'; 2048])], 1_700_000_001_000);
    let blob = &big[0];
    let r = alice.put_bytes(&format!("/v1/repos/acme/web/objects/{}", blob.id()), blob.encode_envelope()).await;
    assert_eq!(r.status, 413, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "too_large");

    // gRPC upload: the stream is refused on the chunk that crosses the limit.
    let ch = Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap();
    let mut objects = ObjectServiceClient::new(ch);
    let envelope = blob.encode_envelope();
    let chunk = v1::UploadObjectsRequest {
        repo: "acme/web".into(),
        chunk: Some(v1::ObjectChunk {
            object_id: blob.id().to_string(),
            total_size: envelope.len() as u64,
            data: envelope,
            last: true,
        }),
    };
    let mut req = tonic::Request::new(tokio_stream::iter(vec![chunk]));
    req.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {}", alice.token)).unwrap());
    let status = objects.upload(req).await.unwrap_err();
    assert_eq!(ag_protocol::ProtocolError::from_status(&status).code, ag_protocol::ErrorCode::TooLarge, "{status}");

    // Workspace edit: the blob the engine would write is held to the same limit.
    workspace(&alice, "acme/web", "w1").await;
    let r = put_file(&alice, "acme/web", "w1", "big.bin", &[b'z'; 4096]).await;
    assert_eq!(r.status, 413, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "too_large");
    assert_eq!(put_file(&alice, "acme/web", "w1", "ok.bin", &[b'z'; 200]).await.status, 200);

    // 0 lifts the repository's own limit again.
    set_quota(&srv, "acme/web", json!({"max_object_bytes": 0})).await;
    let r = alice.put_bytes(&format!("/v1/repos/acme/web/objects/{}", blob.id()), blob.encode_envelope()).await;
    assert_eq!(r.status, 200, "{}", r.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_repositories_start_with_the_configured_quota_defaults() {
    let srv = TestServer::start_with(|cfg| {
        cfg.limits.max_total_object_bytes = 1_000_000;
        cfg.limits.repo_max_object_bytes = 4096;
        cfg.limits.repo_max_streams = 7;
        cfg.limits.max_open_proposals = 3;
        cfg.limits.repo_requests_per_second = 50;
        cfg.limits.repo_burst = 90;
    })
    .await;
    let created = srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    assert_eq!(created.status, 200, "{}", created.json);
    let q = &created.json["quota"];
    assert_eq!(
        (
            q["max_total_object_bytes"].as_u64(),
            q["max_object_bytes"].as_u64(),
            q["max_streams"].as_u64(),
            q["max_open_proposals"].as_u64(),
            q["requests_per_second"].as_u64(),
            q["burst"].as_u64()
        ),
        (Some(1_000_000), Some(4096), Some(7), Some(3), Some(50), Some(90)),
        "{q}"
    );
    // They are the repository's real quota, not just a label: the stream limit applies.
    let admin = srv.admin();
    for i in 0..6 {
        let r = admin.post("/v1/repos/acme/web/streams", json!({"name": format!("s{i}")})).await;
        assert_eq!(r.status, 200, "stream {i}: {}", r.json); // main + 6 = 7
    }
    let r = admin.post("/v1/repos/acme/web/streams", json!({"name": "one-too-many"})).await;
    assert_eq!(r.json["error"]["code"], "quota_exceeded", "{}", r.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unauthenticated_upload_is_refused_before_its_body_is_read() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let addr = srv.base.trim_start_matches("http://").to_string();
    // Announce 100 MB, send 1 KB and wait. A server that buffers the body before it looks at the credentials
    // waits for the rest (this used to hold 256 MiB of memory per anonymous connection); one that checks first
    // answers at once.
    let answer = tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(&addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let oid = format!("{:0>64}", "ab");
        let head = format!(
            "PUT /v1/repos/acme/web/objects/blob_{oid} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/octet-stream\r\nContent-Length: 100000000\r\nConnection: close\r\n\r\n"
        );
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(&[0u8; 1024]).unwrap();
        let mut buf = vec![0u8; 4096];
        let n = s.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    })
    .await
    .unwrap();
    assert!(answer.starts_with("HTTP/1.1 401"), "the answer came before the body did: {answer:?}");
    srv.stop().await;
}
