//! The browser migration path uses a real bundle and publishes no half-imported repository.
#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::json;
use std::{fs, process::Command};
fn bundle() -> Vec<u8> {
    make_bundle(false)
}
fn make_bundle(lfs: bool) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-qb", "main"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "user.name", "Test"],
    ] {
        assert!(Command::new("git").arg("-C").arg(d.path()).args(args).status().unwrap().success());
    }
    fs::write(
        d.path().join("hello.txt"),
        if lfs {
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123\n", "a".repeat(64))
        } else {
            "hello from GitHub or GitLab\n".into()
        },
    )
    .unwrap();
    for args in [
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "hello"],
        vec!["branch", "feature"],
        vec!["tag", "v1"],
    ] {
        assert!(Command::new("git").arg("-C").arg(d.path()).args(args).status().unwrap().success());
    }
    let p = d.path().join("source.bundle");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(d.path())
            .args(["bundle", "create"])
            .arg(&p)
            .arg("--all")
            .status()
            .unwrap()
            .success()
    );
    fs::read(p).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_is_preflighted_then_published_once_with_all_refs() {
    let s = TestServer::start().await;
    let admin = s.admin();
    let plan = admin.put_bytes("/v1/migrations/git?name=acme/moved", bundle()).await;
    assert_eq!(plan.status, 200, "{}", plan.json);
    assert_eq!(plan.json["report"]["branches"].as_array().unwrap().len(), 2);
    assert_eq!(admin.get("/v1/repos/acme/moved").await.status, 404);
    let id = plan.json["id"].as_str().unwrap();
    let path = format!("/v1/migrations/{id}/commit");
    let done = admin.post(&path, json!({"accept_lossy":false})).await;
    assert_eq!(done.status, 200, "{}", done.json);
    assert_eq!(done.json["state"], "imported");
    assert_eq!(admin.post(&path, json!({"accept_lossy":false})).await.json, done.json);
    assert_eq!(admin.get("/v1/repos/acme/moved/streams").await.json["streams"].as_array().unwrap().len(), 2);
    let other = user_with_role(&s, "user:other-admin", "acme/*", "admin").await;
    assert_eq!(other.get(&format!("/v1/migrations/{id}")).await.status, 404);
    assert_eq!(other.post(&path, json!({"accept_lossy":true})).await.status, 404);
    let repo = s.app.repos.handle("acme/moved").unwrap();
    assert_eq!(repo.pins().unwrap()[0].name, "git-tag/v1");
    assert!(repo.fsck().unwrap().is_ok());
    s.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_or_unauthenticated_uploads_do_not_create_a_repository() {
    let s = TestServer::start().await;
    assert_eq!(s.client("").put_bytes("/v1/migrations/git?name=acme/moved", bundle()).await.status, 401);
    let bad = s.admin().put_bytes("/v1/migrations/git?name=acme/moved", b"not a bundle".to_vec()).await;
    assert_eq!(bad.status, 400, "{}", bad.json);
    assert_eq!(s.admin().get("/v1/repos/acme/moved").await.status, 404);
    let staging = s.app.cfg.server.data_dir.join("migrations");
    assert!(
        fs::read_dir(staging).unwrap().all(|entry| !entry.unwrap().path().is_dir()),
        "failed imports must not retain uploaded bundles"
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losses_require_acceptance_before_publication() {
    let s = TestServer::start().await;
    let admin = s.admin();
    let plan = admin.put_bytes("/v1/migrations/git?name=acme/lfs", make_bundle(true)).await;
    assert_eq!(plan.status, 200, "{}", plan.json);
    assert_eq!(plan.json["report"]["lfs_pointers"], 1);
    let route = format!("/v1/migrations/{}/commit", plan.json["id"].as_str().unwrap());
    assert_eq!(admin.post(&route, json!({"accept_lossy":false})).await.status, 400);
    assert_eq!(admin.get("/v1/repos/acme/lfs").await.status, 404);
    assert_eq!(admin.post(&route, json!({"accept_lossy":true})).await.status, 200);
    s.stop().await;
}
