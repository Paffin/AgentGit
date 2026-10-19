//! The derive status and the dangling references of every repository reach `/metrics` and `/readyz`.

mod common;

use ag_storage::meta::Param;
use common::*;
use serde_json::json;

fn gauge(metrics: &str, name: &str) -> Option<i64> {
    metrics.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(' ')?.trim().parse().ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn derive_failures_and_dangling_references_are_in_the_metrics_and_on_readyz() {
    let srv = TestServer::start_with(|cfg| cfg.repos.health_scan_interval_secs = 0).await; // scanned by hand
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/web"})).await.status, 200);
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/api"})).await.status, 200);
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let c = Client::new("acme/web");
    let (head, objs) = c.commit(vec![], "user:alice", "p", &[("f.txt", b"one\n")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objs).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": head.to_string(), "title": "t"}))
        .await;
    assert_eq!(p.status, 200, "{}", p.json);
    let proposal = p.json["id"].as_str().unwrap().to_string();

    // Before any scan nothing is claimed, and a healthy installation says plain "ready".
    assert_eq!(gauge(&srv.app.metrics.render(), "agwp_health_scan_timestamp_seconds"), Some(0));
    let clean = srv.app.refresh_health().await.expect("a scan");
    assert_eq!(
        (clean.repos, clean.unreadable, clean.derive_pending, clean.dangling_total()),
        (2, 0, 0, 0),
        "{clean:?}"
    );
    let metrics = srv.app.metrics.render();
    assert!(gauge(&metrics, "agwp_health_scan_timestamp_seconds").unwrap() > 0, "{metrics}");
    assert_eq!(gauge(&metrics, r#"agwp_dangling_refs{kind="dangling_proposal_head"}"#), Some(0), "{metrics}");
    assert_eq!(srv.client("").get("/readyz").await.body, b"ready");

    // The open proposal is repointed at a changeset that never existed, and a derive pass is recorded as failed
    // with an event still pending.
    let ghost = "ab".repeat(32);
    let web = srv.app.repos.handle("acme/web").unwrap();
    web.meta()
        .write(|tx| {
            tx.raw_execute(
                "UPDATE proposals SET head = ?1 WHERE id = ?2",
                &[Param::from(ghost.as_str()), Param::from(proposal.as_str())],
            )?;
            tx.meta_set("derive.last_error", "caching the tree: disk full")
        })
        .unwrap();
    let found = srv.app.refresh_health().await.expect("a scan");
    assert_eq!(found.dangling.get("dangling_proposal_head"), Some(&1), "{found:?}");
    assert_eq!(found.derive_failing, 1);

    let metrics = srv.app.metrics.render();
    assert_eq!(gauge(&metrics, r#"agwp_dangling_refs{kind="dangling_proposal_head"}"#), Some(1), "{metrics}");
    assert_eq!(gauge(&metrics, r#"agwp_dangling_refs{kind="dangling_pin"}"#), Some(0));
    assert_eq!(gauge(&metrics, "agwp_derive_failing_repositories"), Some(1));
    let ready = srv.client("").get("/readyz").await;
    assert_eq!(ready.status, 200, "attention is not unreadiness");
    let text = String::from_utf8_lossy(&ready.body).into_owned();
    assert!(text.starts_with("ready; needs attention:") && text.contains("1 dangling_proposal_head"), "{text}");

    // Fixed: the next scan says so.
    web.meta()
        .write(|tx| {
            tx.raw_execute(
                "UPDATE proposals SET head = ?1 WHERE id = ?2",
                &[Param::from(head.to_hex()), Param::from(proposal.as_str())],
            )?;
            tx.meta_set("derive.last_error", "")
        })
        .unwrap();
    srv.app.refresh_health().await.expect("a scan");
    assert_eq!(gauge(&srv.app.metrics.render(), r#"agwp_dangling_refs{kind="dangling_proposal_head"}"#), Some(0));
    assert_eq!(srv.client("").get("/readyz").await.body, b"ready");
    srv.stop().await;
}
