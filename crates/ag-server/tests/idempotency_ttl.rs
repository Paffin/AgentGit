//! Idempotency records are kept for `repos.idempotency_ttl_secs`: inside it a repeated request replays the first
//! answer, after it the key is free, and the periodic sweep removes what has expired.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;

const KEY: [(&str, &str); 1] = [("idempotency-key", "make-it-once")];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_replay_inside_their_lifetime_and_are_gone_after_it() {
    let srv = TestServer::start_with(|cfg| {
        cfg.repos.idempotency_ttl_secs = 2;
        cfg.repos.idempotency_sweep_interval_secs = 0; // swept by hand below
    })
    .await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/web"})).await.status, 200);
    let body = || Some(json!({"title": "one task"}));
    let make = || admin.call("POST", "/v1/repos/acme/web/tasks", body(), &KEY);

    let first = make().await;
    assert_eq!(first.status, 200, "{}", first.json);
    let again = make().await;
    assert_eq!(again.json["id"], first.json["id"], "inside the lifetime the answer is replayed");
    let different = admin.call("POST", "/v1/repos/acme/web/tasks", Some(json!({"title": "other"})), &KEY).await;
    assert_eq!(different.json["error"]["code"], "idempotency_key_reused", "{}", different.json);

    let repo = srv.app.repos.handle("acme/web").unwrap();
    let keys = |repo: &ag_integrate::Repository| {
        repo.meta().read(|tx| tx.meta_keys_with_value_below("idem", "~", 100)).unwrap().len()
    };
    assert_eq!(keys(&repo), 2, "a record and its time");

    tokio::time::sleep(Duration::from_millis(2300)).await;
    // Expired: the key can be used again, for the same request and for another one, and makes a new task.
    let third = make().await;
    assert_eq!(third.status, 200, "{}", third.json);
    assert_ne!(third.json["id"], first.json["id"], "after the lifetime the key is free: {}", third.json);
    assert_eq!(make().await.json["id"], third.json["id"], "and the new record replays again");

    // The sweep removes what has expired and nothing else.
    assert_eq!(srv.app.sweep_idempotency_records().await, 0, "the new record is alive");
    admin.call("POST", "/v1/repos/acme/web/tasks", body(), &[("idempotency-key", "short-lived")]).await;
    assert_eq!(keys(&repo), 4);
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert_eq!(srv.app.sweep_idempotency_records().await, 2, "both records are past their lifetime");
    assert_eq!(keys(&repo), 0, "records and their times are gone");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lifetime_of_zero_keeps_records_forever() {
    let srv = TestServer::start_with(|cfg| cfg.repos.idempotency_ttl_secs = 0).await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/web"})).await.status, 200);
    let make = || admin.call("POST", "/v1/repos/acme/web/tasks", Some(json!({"title": "t"})), &KEY);
    let first = make().await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(make().await.json["id"], first.json["id"]);
    assert_eq!(srv.app.sweep_idempotency_records().await, 0);
    srv.stop().await;
}
