//! The server on PostgreSQL metadata and S3 objects.
//!
//! These run only when `AG_TEST_POSTGRES_URL` / `AG_TEST_S3_ENDPOINT` are set (see
//! `scripts/pg-test.sh` and `scripts/s3-test.sh`); with `AG_REQUIRE_BACKENDS=1` a missing
//! variable fails the test instead of skipping it.

mod common;

use ag_integrate::{MetaConfig, ObjectsConfig, StorageConfig};
use common::*;
use serde_json::json;

fn s3_template() -> Option<ObjectsConfig> {
    let endpoint = ag_storage::testing::s3_endpoint()?;
    let var = |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
    Some(ObjectsConfig::S3 {
        endpoint,
        region: var("AG_TEST_S3_REGION", "us-east-1"),
        bucket: var("AG_TEST_S3_BUCKET", "ag-test"),
        prefix: Some(ag_storage::testing::unique("srv")),
        path_style: Some(true),
        access_key_id_env: Some("AG_TEST_S3_ACCESS_KEY_ID".into()),
        secret_access_key_env: Some("AG_TEST_S3_SECRET_ACCESS_KEY".into()),
        session_token_env: None,
        ca_bundle: None,
        lock: Default::default(),
        request_timeout_secs: None,
        max_retries: None,
        list_page_size: None,
    })
}

/// Creating and listing a repository report the storage it really has (they used to say sqlite/fs for all).
async fn exercise(srv: &TestServer, repo: &str, backends: (&str, &str)) {
    let admin = srv.admin();
    let r = admin.post("/v1/repos", json!({"name": repo})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(
        (r.json["meta_backend"].as_str(), r.json["object_backend"].as_str()),
        (Some(backends.0), Some(backends.1))
    );
    let list = admin.get("/v1/repos").await;
    let listed = list.json["repos"].as_array().unwrap().iter().find(|r| r["name"] == repo).expect("listed");
    assert_eq!(
        (listed["meta_backend"].as_str(), listed["object_backend"].as_str()),
        (Some(backends.0), Some(backends.1))
    );
    let alice = user_with_role(srv, "user:alice", repo, "writer").await;
    let carol = user_with_role(srv, "user:carol", repo, "integrator").await;
    let c = Client::new(repo);
    let (head, objs) = c.commit(vec![], "user:alice", "first", &[("a.txt", b"stored remotely\n")], 1_700_000_000_000);
    upload(&alice, repo, &objs).await;
    let p = alice
        .post(
            &format!("/v1/repos/{repo}/proposals"),
            json!({"stream": "main", "head": head.to_string(), "title": "first"}),
        )
        .await;
    assert_eq!(p.status, 200, "{}", p.json);
    let id = p.json["id"].as_str().unwrap();
    let res = carol.post(&format!("/v1/repos/{repo}/proposals/{id}/integrate"), json!({})).await;
    assert_eq!(res.json["outcome"]["status"], "integrated", "{}", res.json);
    let f = alice.get(&format!("/v1/repos/{repo}/files/a.txt?rev=main")).await;
    assert_eq!(f.body, b"stored remotely\n");
    // Integrity checks run against the real backends.
    let fsck = admin.post(&format!("/v1/repos/{repo}/fsck"), json!({})).await;
    assert_eq!(fsck.json["ok"], true, "{}", fsck.json);
    let d = admin.get(&format!("/v1/repos/{repo}")).await;
    assert_eq!(
        (d.json["meta_backend"].as_str(), d.json["object_backend"].as_str()),
        (Some(backends.0), Some(backends.1))
    );
    // The listing counts what the repository holds, on this backend too, and agrees with describe.
    let list = admin.get("/v1/repos").await;
    let listed = list.json["repos"].as_array().unwrap().iter().find(|r| r["name"] == repo).expect("listed");
    assert_eq!(listed["stream_count"], 1, "{listed}");
    assert!(listed["changeset_count"].as_u64().unwrap() >= 1, "{listed}");
    assert_eq!(listed["counts_error"].as_str().unwrap_or(""), "", "{listed}");
    assert_eq!(
        (listed["stream_count"].clone(), listed["changeset_count"].clone()),
        (d.json["stream_count"].clone(), d.json["changeset_count"].clone())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_runs_on_postgres() {
    let Some(url) = ag_storage::testing::postgres_url() else { return };
    // The template reads the URL from the environment variable it names.
    let schema = ag_storage::testing::unique("agsrv");
    let template = StorageConfig {
        meta: MetaConfig::Postgres {
            url_env: Some("AG_TEST_POSTGRES_URL".into()),
            url: None,
            schema: schema.clone(),
            pool_size: Some(4),
        },
        objects: ObjectsConfig::Fs,
    };
    let srv = TestServer::start_with(move |cfg| cfg.repos.storage = Some(template)).await;
    exercise(&srv, "acme/pg", ("postgres", "fs")).await;
    count_quotas_hold_under_concurrent_creates(&srv, "acme/pg").await;
    let d = srv.admin().get("/v1/repos/acme/pg").await;
    assert_eq!(d.json["meta_backend"], "postgres");
    srv.stop().await;
    let name = format!("{schema}_acme_pg");
    let _ = tokio::task::spawn_blocking(move || ag_storage::meta::drop_schema(&url, &name)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_runs_on_s3_objects() {
    let Some(objects) = s3_template() else { return };
    let template = StorageConfig { meta: MetaConfig::Sqlite, objects };
    let srv = TestServer::start_with(move |cfg| cfg.repos.storage = Some(template)).await;
    exercise(&srv, "acme/s3", ("sqlite", "s3")).await;
    let d = srv.admin().get("/v1/repos/acme/s3").await;
    assert_eq!(d.json["object_backend"], "s3");
    srv.stop().await;
}
