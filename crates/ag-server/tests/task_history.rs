//! Task history remains reachable after newer unrelated activity and preserves read permissions.
mod common;
use ag_crypto::KeyPair;
use common::*;
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn task_history_is_filtered_before_paging_and_keeps_repository_permissions() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let repo = "acme/history";
    let base = "/v1/repos/acme/history";
    assert_eq!(admin.post("/v1/repos", json!({"name":repo})).await.status, 200);
    let task = admin.post(&format!("{base}/tasks"), json!({"title":"Old task"})).await;
    assert_eq!(task.status, 200, "{}", task.json);
    let task_id = task.json["id"].as_str().unwrap();
    admin
        .post(
            &format!("{base}/agents"),
            json!({"name":"bot", "public_key":KeyPair::generate().unwrap().public().to_hex()}),
        )
        .await;
    let mut wanted = BTreeSet::new();
    for _ in 0..3 {
        let run = admin.post(&format!("{base}/runs"), json!({"agent":"bot", "task_id":task_id})).await;
        assert_eq!(run.status, 200, "{}", run.json);
        let id = run.json["run_id"].as_str().unwrap();
        wanted.insert(id.to_owned());
        assert_eq!(admin.post(&format!("{base}/runs/{id}/complete"), json!({"status":"completed"})).await.status, 200);
    }
    for _ in 0..5 {
        assert_eq!(admin.post(&format!("{base}/runs"), json!({"agent":"bot"})).await.status, 200);
    }
    let reader = user_with_role(&srv, "user:reader", repo, "reader").await;
    let mut token = String::new();
    let mut seen = BTreeSet::new();
    loop {
        let page = reader.get(&format!("{base}/runs?task_id={task_id}&limit=1&page_token={token}")).await;
        assert_eq!(page.status, 200, "{}", page.json);
        let rows = page.json["runs"].as_array().unwrap();
        assert!(rows.len() <= 1);
        for row in rows {
            assert_eq!(row["task_id"], task_id);
            assert!(seen.insert(row["run_id"].as_str().unwrap().to_owned()), "page repeated a run");
        }
        token = page.json["next_page_token"].as_str().unwrap_or("").to_owned();
        if token.is_empty() {
            break;
        }
    }
    assert_eq!(seen, wanted);
    assert_eq!(reader.get(&format!("{base}/runs?task_id={task_id}&active_only=true")).await.json["runs"], json!([]));
    assert_eq!(reader.get(&format!("{base}/runs?page_token=invalid")).await.status, 400);
    let outsider = user_with_role(&srv, "user:outsider", "other/repository", "reader").await;
    assert_eq!(outsider.get(&format!("{base}/runs?task_id={task_id}")).await.status, 404);

    let client = Client::new(repo);
    let (head, objects) =
        client.commit(vec![], "user:author", "history", &[("file.txt", b"content\n")], 1_700_000_000_000);
    upload(&admin, repo, &objects).await;
    let mut linked = BTreeSet::new();
    for n in 0..8 {
        let response = admin.post(&format!("{base}/proposals"), json!({"stream":"main", "head":head.to_string(), "title":format!("Change {n}"), "task_id":if n < 3 {task_id} else {""}})).await;
        assert_eq!(response.status, 200, "{}", response.json);
        if n < 3 {
            linked.insert(response.json["id"].as_str().unwrap().to_owned());
        }
    }
    token.clear();
    seen.clear();
    loop {
        let page = reader.get(&format!("{base}/proposals?task_id={task_id}&page_size=1&page_token={token}")).await;
        assert_eq!(page.status, 200, "{}", page.json);
        let rows = page.json["proposals"].as_array().unwrap();
        assert!(rows.len() <= 1);
        for row in rows {
            assert!(seen.insert(row["id"].as_str().unwrap().to_owned()));
        }
        token = page.json["next_page_token"].as_str().unwrap_or("").to_owned();
        if token.is_empty() {
            break;
        }
    }
    assert_eq!(seen, linked);
    assert_eq!(outsider.get(&format!("{base}/proposals?task_id={task_id}")).await.status, 404);
    srv.stop().await;
}
