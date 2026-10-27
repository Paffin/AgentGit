//! Integrations racing on one stream: the server retries the lost compare-and-swaps itself
//! (`repos.integrate_attempts`, `repos.integrate_backoff_ms`) instead of answering `aborted` for most of them.

mod common;

use common::*;
use serde_json::json;

const WRITERS: usize = 16;

/// Opens `WRITERS` proposals on `main`, each adding a file of its own on one base, then integrates all of them at
/// once. Returns the answers' `outcome.status` (or the error code) and the stream's final version.
async fn race(srv: &TestServer) -> (Vec<String>, u64) {
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/web"})).await.status, 200);
    let alice = user_with_role(srv, "user:alice", "acme/web", "writer").await;
    let c = Client::new("acme/web");
    let (base, objects) = c.commit(vec![], "user:alice", "base", &[("base.txt", b"0\n")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objects).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": base.to_string(), "title": "base"}))
        .await;
    let id = p.json["id"].as_str().unwrap().to_string();
    assert_eq!(admin.post(&format!("/v1/repos/acme/web/proposals/{id}/integrate"), json!({})).await.status, 200);

    let mut proposals = Vec::new();
    for i in 0..WRITERS {
        let name = format!("w{i}.txt");
        let (head, objects) = c.commit(
            vec![base],
            "user:alice",
            &format!("w{i}"),
            &[(name.as_str(), b"x\n")],
            1_700_000_000_001 + i as i64,
        );
        upload(&alice, "acme/web", &objects).await;
        let p = alice
            .post(
                "/v1/repos/acme/web/proposals",
                json!({"stream": "main", "head": head.to_string(), "title": format!("w{i}")}),
            )
            .await;
        assert_eq!(p.status, 200, "{}", p.json);
        proposals.push(p.json["id"].as_str().unwrap().to_string());
    }
    let racing: Vec<_> = proposals
        .into_iter()
        .map(|id| {
            let admin = admin.clone();
            tokio::spawn(async move {
                let r = admin.post(&format!("/v1/repos/acme/web/proposals/{id}/integrate"), json!({})).await;
                match r.json["outcome"]["status"].as_str() {
                    Some(status) => status.to_string(),
                    None => r.json["error"]["code"].as_str().unwrap_or("?").to_string(),
                }
            })
        })
        .collect();
    let mut answers = Vec::new();
    for h in racing {
        answers.push(h.await.unwrap());
    }
    let version =
        admin.get("/v1/repos/acme/web/streams/head?stream=main").await.json["version"].as_u64().expect("a version");
    (answers, version)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn writers_racing_on_one_stream_all_land_with_the_default_retry() {
    let srv = TestServer::start().await;
    let (answers, version) = race(&srv).await;
    assert!(answers.iter().all(|a| a == "integrated"), "{answers:?}");
    assert_eq!(version, 1 + WRITERS as u64, "the base and every writer, each exactly once");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_attempt_is_all_a_loser_gets_and_the_stream_still_ends_up_consistent() {
    let srv = TestServer::start_with(|cfg| cfg.repos.integrate_attempts = 1).await;
    let (answers, version) = race(&srv).await;
    let landed = answers.iter().filter(|a| *a == "integrated").count() as u64;
    assert!(answers.iter().all(|a| a == "integrated" || a == "aborted"), "{answers:?}");
    assert!(landed >= 1, "somebody wins every race: {answers:?}");
    assert!(
        landed < WRITERS as u64,
        "with one attempt the losers are told to retry (the setting is applied): {answers:?}"
    );
    assert_eq!(version, 1 + landed, "an aborted integration changed nothing: {answers:?}");
    srv.stop().await;
}
