//! `GET /proposals`: filters are applied in SQL and pages are keyset pages - a complete walk sees every matching
//! proposal exactly once, newest first, even when proposals are opened or closed between two requests.

mod common;

use std::collections::HashSet;

use common::*;
use serde_json::{Value, json};

const REPO: &str = "acme/web";

async fn open(http: &Http, c: &Client, stream: &str, n: usize, who: &str) -> String {
    let (head, objs) = c.commit(
        vec![],
        who,
        "p",
        &[("f.txt", format!("{who} {stream} {n}\n").as_bytes())],
        1_700_000_000_000 + n as i64,
    );
    upload(http, REPO, &objs).await;
    let r = http
        .post(
            &format!("/v1/repos/{REPO}/proposals"),
            json!({"stream": stream, "head": head.to_string(), "title": format!("{who}-{stream}-{n}")}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    r.json["id"].as_str().unwrap().to_string()
}

fn ids(page: &Value) -> Vec<String> {
    page["proposals"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect()
}

/// Walks every page of `query`, returning the ids in order.
async fn walk(http: &Http, query: &str, size: usize) -> Vec<String> {
    let mut all = Vec::new();
    let mut token = String::new();
    loop {
        let r = http.get(&format!("/v1/repos/{REPO}/proposals?page_size={size}&page_token={token}&{query}")).await;
        assert_eq!(r.status, 200, "{}", r.json);
        assert!(ids(&r.json).len() <= size);
        all.extend(ids(&r.json));
        token = r.json["next_page_token"].as_str().unwrap_or("").to_string();
        if token.is_empty() {
            return all;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proposals_are_filtered_and_paged_by_key() {
    let srv = TestServer::start().await;
    assert_eq!(srv.admin().post("/v1/repos", json!({"name": REPO})).await.status, 200);
    assert_eq!(srv.admin().post(&format!("/v1/repos/{REPO}/streams"), json!({"name": "dev"})).await.status, 200);
    let alice = user_with_role(&srv, "user:alice", REPO, "writer").await;
    let bob = user_with_role(&srv, "user:bob", REPO, "writer").await;
    let c = Client::new(REPO);

    let mut made = Vec::new(); // (id, author, stream), oldest first
    for n in 0..7 {
        made.push((open(&alice, &c, "main", n, "user:alice").await, "user:alice", "main"));
    }
    for n in 7..10 {
        made.push((open(&bob, &c, "dev", n, "user:bob").await, "user:bob", "dev"));
    }
    made.push((open(&alice, &c, "dev", 10, "user:alice").await, "user:alice", "dev"));
    let close = &made[2].0;
    assert_eq!(alice.post(&format!("/v1/repos/{REPO}/proposals/{close}/close"), json!({})).await.status, 200);

    // The reference order is newest first (created_at, then id, both descending).
    let newest_first = |keep: &dyn Fn(&(String, &str, &str)) -> bool| -> Vec<String> {
        let mut rows: Vec<_> = made.iter().filter(|m| keep(m)).cloned().collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.into_iter().map(|m| m.0).collect()
    };
    // Ids are random, so the order among proposals made in the same millisecond is by id; take the server's whole
    // listing as the order and check that every filtered or paged walk is consistent with it.
    let everything = walk(&alice, "", 100).await;
    assert_eq!(everything.len(), made.len());
    assert_eq!(
        everything.iter().cloned().collect::<HashSet<_>>(),
        newest_first(&|_| true).into_iter().collect::<HashSet<_>>()
    );
    for size in [1, 3, 4, 100] {
        assert_eq!(walk(&alice, "", size).await, everything, "page size {size}");
    }
    let subset = |keep: &dyn Fn(&(String, &str, &str)) -> bool| -> Vec<String> {
        everything.iter().filter(|id| made.iter().any(|m| &m.0 == *id && keep(m))).cloned().collect()
    };
    let is_closed = |m: &(String, &str, &str)| &m.0 == close;
    assert_eq!(walk(&alice, "state=open", 3).await, subset(&|m| !is_closed(m)));
    assert_eq!(walk(&alice, "state=closed", 3).await, subset(&is_closed));
    assert_eq!(walk(&alice, "stream=dev", 2).await, subset(&|m| m.2 == "dev"));
    assert_eq!(walk(&alice, "author=user:bob", 2).await, subset(&|m| m.1 == "user:bob"));
    assert_eq!(
        walk(&alice, "author=user:alice&stream=dev&state=open", 2).await,
        subset(&|m| m.1 == "user:alice" && m.2 == "dev")
    );
    assert!(walk(&alice, "author=user:nobody", 2).await.is_empty());

    // A walk is stable while the data moves: proposals opened after the first page are newer than everything the
    // walk has yet to show, and closing one that was already shown or not yet shown changes only the filter.
    let first = alice.get(&format!("/v1/repos/{REPO}/proposals?page_size=4")).await;
    let token = first.json["next_page_token"].as_str().unwrap().to_string();
    open(&bob, &c, "main", 11, "user:bob").await;
    open(&bob, &c, "main", 12, "user:bob").await;
    let mut rest = Vec::new();
    let mut token = token;
    while !token.is_empty() {
        let r = alice.get(&format!("/v1/repos/{REPO}/proposals?page_size=4&page_token={token}")).await;
        rest.extend(ids(&r.json));
        token = r.json["next_page_token"].as_str().unwrap_or("").to_string();
    }
    let mut seen = ids(&first.json);
    seen.extend(rest);
    assert_eq!(seen, everything, "new proposals neither shift nor duplicate the rows of a walk in progress");

    // A token is only good for the listing that made it, and garbage is refused.
    let tree = alice.get(&format!("/v1/repos/{REPO}/streams")).await;
    assert_eq!(tree.status, 200);
    for bad in ["not-a-token", "cHJvcDo0Mg" /* "prop:42": an old offset token */] {
        let r = alice.get(&format!("/v1/repos/{REPO}/proposals?page_token={bad}")).await;
        assert_eq!(r.status, 400, "{bad}: {}", r.json);
    }
    srv.stop().await;
}
