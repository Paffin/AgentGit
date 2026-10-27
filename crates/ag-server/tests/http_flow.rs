//! End-to-end over real sockets: a team, an agent and CI share one repository.

mod common;

use common::*;
use serde_json::json;

#[tokio::test]
async fn capabilities_are_public_and_everything_else_needs_credentials() {
    let srv = TestServer::start().await;
    let anon = srv.client("");
    let r = anon.get("/v1/capabilities").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json["protocol_version"], "agwp.v1");
    assert!(r.json["features"].as_array().unwrap().iter().any(|f| f == "events.replay"));
    // The advertised changeset limits are the engine's, not a second copy of them.
    let engine = ag_types::Limits::default();
    assert_eq!(r.json["limits"]["max_changeset_ops"], engine.max_ops);
    assert_eq!(r.json["limits"]["max_changesets_per_publication"], engine.max_changesets_per_publication);

    let r = anon.get("/v1/whoami").await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json["error"]["code"], "unauthenticated");
    assert!(r.json["error"]["request_id"].as_str().unwrap().starts_with("req_"));

    let r = srv.client("agt_0000000000000000_nope").get("/v1/whoami").await;
    assert_eq!(r.status, 401);

    let r = srv.admin().get("/v1/whoami").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json["method"], "bootstrap");

    assert_eq!(anon.get("/healthz").await.status, 200);
    assert_eq!(anon.get("/readyz").await.status, 200);
    let spec = anon.get("/openapi.json").await;
    assert_eq!(spec.status, 200);
    assert_eq!(spec.json["openapi"], "3.1.0");
    srv.stop().await;
}

#[tokio::test]
async fn a_change_travels_from_upload_to_integration() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let r = admin.post("/v1/repos", json!({"name": "acme/web", "description": "demo"})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["default_stream"], "main");

    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let bob = user_with_role(&srv, "user:bob", "acme/web", "reviewer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;

    // Policy: one approval required.
    let r = admin
        .call(
            "PUT",
            "/v1/repos/acme/web/policy",
            Some(json!({"stream": "main", "document": r#"{"approvals":{"min":1}}"#})),
            &[],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);

    // Alice publishes a root change and opens a proposal.
    let c = Client::new("acme/web");
    let (head, objects) = c.commit(
        vec![],
        "user:alice",
        "initial",
        &[("README.md", b"# acme\n"), ("src/lib.rs", b"fn main() {}\n")],
        1_700_000_000_000,
    );
    upload(&alice, "acme/web", &objects).await;
    let r = alice
        .post(
            "/v1/repos/acme/web/proposals",
            json!({"stream": "main", "head": head.to_string(), "title": "Initial import", "idempotency_key": "p1"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let prop = r.json["id"].as_str().unwrap().to_string();
    // Retrying with the same key returns the same proposal.
    let again = alice
        .post(
            "/v1/repos/acme/web/proposals",
            json!({"stream": "main", "head": head.to_string(), "title": "Initial import", "idempotency_key": "p1"}),
        )
        .await;
    assert_eq!(again.json["id"], prop.as_str());
    // The same key with different content is refused.
    let reused = alice
        .post(
            "/v1/repos/acme/web/proposals",
            json!({"stream": "main", "head": head.to_string(), "title": "Something else", "idempotency_key": "p1"}),
        )
        .await;
    assert_eq!(reused.status, 422);
    assert_eq!(reused.json["error"]["code"], "idempotency_key_reused");

    // Separation of duties: the author cannot integrate, nor review.
    let r = alice.post(&format!("/v1/repos/acme/web/proposals/{prop}/integrate"), json!({})).await;
    assert_eq!(r.status, 403, "{}", r.json);
    let r = alice.post(&format!("/v1/repos/acme/web/proposals/{prop}/reviews"), json!({"decision": "approve"})).await;
    assert_eq!(r.status, 403);

    // Carol integrates before approval: blocked with a precise reason, stream untouched.
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{prop}/integrate"), json!({})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["outcome"]["status"], "blocked");
    assert!(r.json["outcome"]["blockers"][0]["message"].as_str().unwrap().contains("approvals"));
    let head_now = carol.get("/v1/repos/acme/web/streams/head?stream=main").await;
    assert_eq!(head_now.json["head"], "");

    // The prepare endpoint tells the same story without side effects.
    let prep = carol.post(&format!("/v1/repos/acme/web/proposals/{prop}/prepare"), json!({})).await;
    assert_eq!(prep.status, 200, "{}", prep.json);
    assert_eq!(prep.json["strategy"], "fast_forward");
    assert_eq!(prep.json["blockers"].as_array().unwrap().len(), 1);

    // Bob approves; Carol integrates.
    let r = bob
        .post(&format!("/v1/repos/acme/web/proposals/{prop}/reviews"), json!({"decision": "approve", "body": "lgtm"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let r = carol
        .post(&format!("/v1/repos/acme/web/proposals/{prop}/integrate"), json!({"idempotency_key": "int-1"}))
        .await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
    assert_eq!(r.json["outcome"]["new_head"], head.to_string());
    // A lost response is recoverable by asking for the operation.
    let op = carol.get("/v1/repos/acme/web/operations?idempotency_key=int-1").await;
    assert_eq!(op.status, 200, "{}", op.json);
    assert_eq!(op.json["integration"]["outcome"]["status"], "integrated");

    // Reading it back: head, tree, file content, log, diff.
    let head_now = alice.get("/v1/repos/acme/web/streams/head?stream=main").await;
    assert_eq!(head_now.json["head"], head.to_string());
    assert_eq!(head_now.json["version"], 1);
    let file = alice.get("/v1/repos/acme/web/files/src/lib.rs?rev=main").await;
    assert_eq!(file.status, 200);
    assert_eq!(file.body, b"fn main() {}\n");
    let list = alice.get("/v1/repos/acme/web/tree/list?rev=main").await;
    let names: Vec<&str> =
        list.json["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["README.md", "src"]);
    let log = alice.get("/v1/repos/acme/web/log?rev=main").await;
    assert_eq!(log.json["changesets"][0]["message"], "initial");
    let diff = alice.post("/v1/repos/acme/web/diff", json!({"to": "main"})).await;
    assert_eq!(diff.json["files"].as_array().unwrap().len(), 2);
    assert_eq!(diff.json["files"][0]["status"], "added");

    // Everything above is in the replayable event feed, in order.
    let ev = alice.get("/v1/repos/acme/web/events/replay?after_seq=0&limit=100").await;
    let topics: Vec<&str> =
        ev.json["events"].as_array().unwrap().iter().map(|e| e["topic"].as_str().unwrap()).collect();
    assert!(topics.contains(&"proposal.create") && topics.contains(&"stream.integrate"), "{topics:?}");
    let seqs: Vec<u64> = ev.json["events"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    let audit = admin.get("/v1/repos/acme/web/audit/verify").await;
    assert_eq!(audit.json["ok"], true);
    srv.stop().await;
}

#[tokio::test]
async fn private_repositories_do_not_reveal_their_existence() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/secret"})).await;
    let outsider = user_with_role(&srv, "user:mallory", "other/*", "writer").await;
    for path in ["/v1/repos/acme/secret", "/v1/repos/acme/secret/streams", "/v1/repos/acme/does-not-exist"] {
        let r = outsider.get(path).await;
        assert_eq!(r.status, 404, "{path}: {}", r.json);
        assert_eq!(r.json["error"]["code"], "not_found");
    }
    let list = outsider.get("/v1/repos").await;
    assert_eq!(list.json["repos"].as_array().unwrap().len(), 0);
    let r = outsider.post("/v1/repos", json!({"name": "acme/mine"})).await;
    assert_eq!(r.status, 403, "creating repositories outside one's own pattern is denied");
    srv.stop().await;
}

#[tokio::test]
async fn the_server_enforces_client_deadlines() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/slow"})).await;
    // Long-poll for a version that never comes, with a 300 ms deadline.
    let started = std::time::Instant::now();
    let r = admin
        .call(
            "GET",
            "/v1/repos/acme/slow/streams/head?stream=main&wait_version_gt=99&wait_ms=20000",
            None,
            &[("request-timeout", "300")],
        )
        .await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "the deadline must cut the wait short");
    assert_eq!(r.status, 504, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "deadline_exceeded");
    assert!(r.json["error"]["retryable"].as_bool().unwrap());

    // Without a deadline the long-poll returns when the wait elapses, with the current state.
    let r = admin.get("/v1/repos/acme/slow/streams/head?stream=main&wait_version_gt=99&wait_ms=200").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json["version"], 0);
    srv.stop().await;
}

#[tokio::test]
async fn a_review_cannot_approve_a_head_the_reviewer_did_not_see() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/review-head"})).await.status, 200);
    let author = user_with_role(&srv, "user:author", "acme/review-head", "writer").await;
    let reviewer = user_with_role(&srv, "user:reviewer", "acme/review-head", "reviewer").await;
    let c = Client::new("acme/review-head");
    let (seen, objects) = c.commit(vec![], "user:author", "seen", &[("a", b"one")], 1_700_000_000_000);
    upload(&author, "acme/review-head", &objects).await;
    let (updated, objects) = c.commit(vec![seen], "user:author", "unseen", &[("a", b"two")], 1_700_000_000_001);
    upload(&author, "acme/review-head", &objects).await;
    let proposal = author
        .post(
            "/v1/repos/acme/review-head/proposals",
            json!({"stream": "main", "head": seen.to_string(), "title": "Review this"}),
        )
        .await;
    assert_eq!(proposal.status, 200, "{}", proposal.json);
    let path = format!("/v1/repos/acme/review-head/proposals/{}", proposal.json["id"].as_str().unwrap());
    let changed = author.call("PATCH", &path, Some(json!({"new_head": updated.to_string()})), &[]).await;
    assert_eq!(changed.status, 200, "{}", changed.json);

    for decision in ["approve", "request_changes", "comment"] {
        let stale = reviewer
            .post(
                &format!("{path}/reviews"),
                json!({"decision": decision, "body": "only saw version one", "expected_head": seen.to_string()}),
            )
            .await;
        assert_eq!(stale.status, 412, "{}", stale.json);
        assert_eq!(stale.json["error"]["code"], "failed_precondition");
    }
    let untouched = reviewer.get(&path).await;
    assert_eq!(untouched.json["reviews"].as_array().unwrap().len(), 0);
    let audit = admin.get("/v1/repos/acme/review-head/audit?contains=proposal.review").await;
    assert_eq!(audit.status, 200, "{}", audit.json);
    assert_eq!(audit.json["entries"].as_array().unwrap().len(), 0);
    let malformed = reviewer
        .post(&format!("{path}/reviews"), json!({"decision": "approve", "expected_head": "not-a-changeset"}))
        .await;
    assert_eq!(malformed.status, 400, "{}", malformed.json);

    let accepted = reviewer
        .post(&format!("{path}/reviews"), json!({"decision": "approve", "expected_head": updated.to_string()}))
        .await;
    assert_eq!(accepted.status, 200, "{}", accepted.json);
    assert_eq!(accepted.json["head"], updated.to_string());
    srv.stop().await;
}
