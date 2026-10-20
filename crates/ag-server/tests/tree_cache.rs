//! The decoded-tree cache of the server (`repos.tree_cache_bytes`): integrations are served from memory, the
//! answers are the same with the cache on and off, a small budget evicts, and the counters are on `/metrics`.

mod common;

use common::*;
use serde_json::json;

/// Lands `n` changesets on `main` (each a proposal by alice that admin integrates, the next one built on the
/// previous head), then reads the file back at every landed head.
async fn land_history(srv: &TestServer, n: usize) -> Vec<String> {
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": "acme/web"})).await.status, 200);
    let alice = user_with_role(srv, "user:alice", "acme/web", "writer").await;
    let c = Client::new("acme/web");
    let mut parent = None;
    let mut heads = Vec::new();
    for i in 0..n {
        let files: Vec<(String, Vec<u8>)> =
            (0..=i).map(|j| (format!("src/f{j}.txt"), format!("file {j} at {i}\n").into_bytes())).collect();
        let files: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
        let (head, objects) = c.commit(
            parent.into_iter().collect(),
            "user:alice",
            &format!("c{i}"),
            &files,
            1_700_000_000_000 + i as i64,
        );
        upload(&alice, "acme/web", &objects).await;
        let p = alice
            .post(
                "/v1/repos/acme/web/proposals",
                json!({"stream": "main", "head": head.to_string(), "title": format!("c{i}")}),
            )
            .await;
        assert_eq!(p.status, 200, "{}", p.json);
        let id = p.json["id"].as_str().unwrap().to_string();
        let r = admin.post(&format!("/v1/repos/acme/web/proposals/{id}/integrate"), json!({})).await;
        assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
        parent = Some(head);
        heads.push(head.to_string());
    }
    heads
}

async fn files_at(srv: &TestServer, head: &str) -> serde_json::Value {
    let r = srv.admin().get(&format!("/v1/repos/acme/web/tree?rev={head}&page_size=1000")).await;
    assert_eq!(r.status, 200, "{}", r.json);
    r.json
}

fn metric(text: &str, name: &str) -> f64 {
    text.lines()
        .find_map(|l| l.strip_prefix(name).and_then(|rest| rest.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no metric {name} in:\n{text}"))
        .parse()
        .unwrap()
}

async fn scrape(srv: &TestServer) -> String {
    String::from_utf8(srv.client("").get("/metrics").await.body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trees_are_served_from_memory_and_are_the_same_trees_without_the_cache() {
    let cached = TestServer::start_with(|cfg| cfg.repos.tree_cache_bytes = 64 * 1024 * 1024).await;
    let plain = TestServer::start_with(|cfg| cfg.repos.tree_cache_bytes = 0).await;
    let heads = land_history(&cached, 12).await;
    let heads_plain = land_history(&plain, 12).await;
    assert_eq!(heads, heads_plain, "the same history on both servers");
    // Every head reads back identically with and without the cache (twice: the second read is a hit).
    for head in &heads {
        let a = files_at(&cached, head).await;
        assert_eq!(a, files_at(&plain, head).await, "head {head}");
        assert_eq!(a, files_at(&cached, head).await, "head {head}, from memory");
    }
    let m = scrape(&cached).await;
    assert!(metric(&m, "agwp_tree_cache_hits_total") > 0.0, "{m}");
    assert!(metric(&m, "agwp_tree_cache_entries") > 0.0, "{m}");
    assert!(metric(&m, "agwp_tree_cache_bytes") > 0.0, "{m}");
    let off = scrape(&plain).await;
    assert_eq!(metric(&off, "agwp_tree_cache_hits_total"), 0.0, "{off}");
    assert_eq!(metric(&off, "agwp_tree_cache_entries"), 0.0, "{off}");
    cached.stop().await;
    plain.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_small_budget_evicts_and_the_answers_stay_right() {
    // Room for about two of the larger trees.
    let srv = TestServer::start_with(|cfg| cfg.repos.tree_cache_bytes = 4096).await;
    let heads = land_history(&srv, 14).await;
    for head in &heads {
        let tree = files_at(&srv, head).await;
        let n = heads.iter().position(|h| h == head).unwrap() + 1;
        assert_eq!(tree["total_entries"], n, "head {head}: {tree}");
    }
    let m = scrape(&srv).await;
    assert!(metric(&m, "agwp_tree_cache_evictions_total") > 0.0, "{m}");
    assert!(metric(&m, "agwp_tree_cache_bytes") <= 4096.0, "the budget holds: {m}");
    srv.stop().await;
}
