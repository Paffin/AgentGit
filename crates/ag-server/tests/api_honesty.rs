//! The API says what is true: no invented outcomes, sizes, backends or limits.

mod common;

use ag_integrate::{MetaConfig, ObjectsConfig, RepoConfig, StorageConfig};
use ag_protocol::v1;
use ag_protocol::v1::repository_service_client::RepositoryServiceClient;
use ag_server::control::RepoRow;
use ag_types::{Namespace, StreamName};
use common::*;
use serde_json::json;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

/// Alice proposes `file` on main in `repo`; returns the proposal id and the uploaded objects (blob first).
async fn propose(alice: &Http, repo: &str, file: (&str, &[u8])) -> (String, Vec<ag_object::RawObject>) {
    let c = Client::new(repo);
    let (head, objs) = c.commit(vec![], "user:alice", "first", &[file], 1_700_000_000_000);
    upload(alice, repo, &objs).await;
    let p = alice
        .post(
            &format!("/v1/repos/{repo}/proposals"),
            json!({"stream": "main", "head": head.to_string(), "title": "first"}),
        )
        .await;
    assert_eq!(p.status, 200, "{}", p.json);
    (p.json["id"].as_str().unwrap().to_string(), objs)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_that_has_not_finished_has_no_outcome_yet() {
    // One engine handle per repository, so the failpoint armed below is the one the request meets.
    let srv = TestServer::start_with(|cfg| cfg.repos.pool_size = 1).await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let (pid, _) = propose(&alice, "acme/web", ("a.txt", b"a")).await;

    // The server dies (here: a fault is injected) between preparing the integration and committing it.
    srv.app.repos.handle("acme/web").unwrap().failpoints().arm("integrate.before_commit");
    let lost =
        carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({"idempotency_key": "int-1"})).await;
    assert_eq!(lost.status, 500, "{}", lost.json);

    // What a client does next (as the docs tell it): ask for the outcome. It is not decided yet, and the answer
    // must not claim an outcome - the old one said {"status": "invalid"}, a terminal verdict that never happened.
    let op = carol.get("/v1/repos/acme/web/operations?idempotency_key=int-1").await;
    assert_eq!(op.status, 200, "{}", op.json);
    let state = op.json["integration"]["state"].as_str().unwrap();
    assert!(!["integrated", "rejected", "failed"].contains(&state), "still in flight, not {state}");
    assert!(op.json["integration"]["outcome"].is_null(), "no outcome before there is one: {}", op.json);

    // Retrying with the same key completes it, and now the outcome is there.
    let done =
        carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({"idempotency_key": "int-1"})).await;
    assert_eq!(done.json["outcome"]["status"], "integrated", "{}", done.json);
    let op = carol.get("/v1/repos/acme/web/operations?idempotency_key=int-1").await;
    assert_eq!(op.json["integration"]["outcome"]["status"], "integrated", "{}", op.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_the_tree_names_but_the_store_lost_is_an_error_not_an_empty_file() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let (pid, objs) = propose(&alice, "acme/web", ("a.txt", b"content that will go missing")).await;
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);

    // Healthy: sizes are the real ones.
    let l = alice.get("/v1/repos/acme/web/tree/list?rev=main").await;
    assert_eq!(l.json["entries"][0]["size"], 28, "{}", l.json);
    let t = alice.get("/v1/repos/acme/web/tree?rev=main").await;
    assert_eq!(t.json["entries"][0]["size"], 28);
    let rp = alice.get("/v1/repos/acme/web/tree/resolve?rev=main&path=a.txt").await;
    assert_eq!(rp.json["entry"]["size"], 28);

    // Damage: the object store loses the blob. fsck would say so; so must every listing - not "size 0".
    let handle = srv.app.repos.handle("acme/web").unwrap();
    assert!(handle.objects().delete(&objs[0].id()).unwrap());
    for path in [
        "/v1/repos/acme/web/tree/list?rev=main",
        "/v1/repos/acme/web/tree?rev=main",
        "/v1/repos/acme/web/tree/resolve?rev=main&path=a.txt",
    ] {
        let r = alice.get(path).await;
        assert_eq!(r.status, 422, "{path}: {}", r.json);
        assert_eq!(r.json["error"]["code"], "object_missing", "{path}");
    }
    // fsck names the object (the field used to be an empty string whatever was wrong).
    let fsck = srv.admin().post("/v1/repos/acme/web/fsck", json!({})).await;
    assert_eq!(fsck.json["ok"], false, "{}", fsck.json);
    let issue = fsck.json["issues"].as_array().unwrap().iter().find(|i| i["kind"] == "bad_blob").expect("bad_blob");
    assert_eq!(issue["object"], objs[0].id().to_string(), "{issue}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_queue_counts_every_outcome_not_just_success() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    // Landing needs an approval nobody gives: the queued proposal stays blocked.
    let r = admin
        .call(
            "PUT",
            "/v1/repos/acme/web/policy",
            Some(json!({"stream": "main", "document": r#"{"approvals":{"min":1}}"#})),
            &[],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let (pid, _) = propose(&alice, "acme/web", ("a.txt", b"a")).await;
    let q = carol.post("/v1/repos/acme/web/queue", json!({"proposal": pid})).await;
    assert_eq!(q.status, 200, "{}", q.json);
    let run = carol.post("/v1/repos/acme/web/queue/run", json!({"stream": "main"})).await;
    assert_eq!(run.status, 200, "{}", run.json);
    assert_eq!(run.json["processed"][0]["state"], "blocked", "{}", run.json);
    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    assert!(m.contains("agwp_integrations_total{outcome=\"blocked\"} 1"), "blocked entries are counted: {m}");
    assert!(!m.contains("outcome=\"integrated\""), "{m}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_and_list_report_the_storage_a_repository_really_has() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let created = admin.post("/v1/repos", json!({"name": "acme/plain"})).await;
    assert_eq!(
        (created.json["meta_backend"].as_str(), created.json["object_backend"].as_str()),
        (Some("sqlite"), Some("fs"))
    );

    // A repository whose registry entry says PostgreSQL + S3 (as one created under the shipped Compose
    // configuration is). It is only registered here, never opened: listing must not need the services.
    let config = RepoConfig {
        format: 1,
        namespace: Namespace::new("acme/shared").unwrap(),
        default_stream: StreamName::new("main").unwrap(),
        storage: Some(StorageConfig {
            meta: MetaConfig::Postgres {
                url_env: Some("AG_NOT_SET".into()),
                url: None,
                schema: "ag_acme_shared".into(),
                pool_size: None,
            },
            objects: ObjectsConfig::S3 {
                endpoint: "http://minio.invalid:9000".into(),
                region: "us-east-1".into(),
                bucket: "agentgit".into(),
                prefix: Some("acme/shared".into()),
                path_style: Some(true),
                access_key_id_env: None,
                secret_access_key_env: None,
                session_token_env: None,
                ca_bundle: None,
                lock: Default::default(),
            },
        }),
    };
    srv.app
        .control
        .insert_repo(&RepoRow {
            name: "acme/shared".into(),
            namespace: "acme/shared".into(),
            default_stream: "main".into(),
            description: String::new(),
            created_at_ms: 1,
            created_by: "user:root".into(),
            quota: Default::default(),
            object_bytes: 0,
            config_json: serde_json::to_string(&config).unwrap(),
        })
        .unwrap();

    let list = admin.get("/v1/repos").await;
    assert_eq!(list.status, 200, "{}", list.json);
    let backends: Vec<(String, String, String)> = list.json["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().into(),
                r["meta_backend"].as_str().unwrap().into(),
                r["object_backend"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        backends,
        [
            ("acme/plain".to_string(), "sqlite".to_string(), "fs".to_string()),
            ("acme/shared".to_string(), "postgres".to_string(), "s3".to_string()),
        ],
        "the listing used to say sqlite/fs for everything"
    );
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_limits_the_server_advertises_are_the_ones_the_engine_enforces() {
    let srv = TestServer::start().await;
    srv.admin().post("/v1/repos", json!({"name": "acme/web"})).await;
    let caps = srv.client("").get("/v1/capabilities").await;
    let l = &caps.json["limits"];
    let engine = srv.app.repos.handle("acme/web").unwrap();
    let enforced = engine.limits();
    assert_eq!(l["max_changeset_ops"].as_u64(), Some(enforced.max_ops as u64));
    assert_eq!(l["max_changesets_per_publication"].as_u64(), Some(enforced.max_changesets_per_publication as u64));
    assert_eq!(
        l["max_object_bytes"].as_u64(),
        Some(enforced.max_blob_size + ag_object::ENVELOPE_HEADER_LEN as u64),
        "the largest object the server accepts is the largest the engine can store"
    );
    assert_eq!(l["max_request_bytes"].as_u64(), Some(srv.app.cfg.limits.max_request_bytes));

    // An operator who sets a larger object limit than the engine can store is not believed.
    let srv2 = TestServer::start_with(|cfg| cfg.limits.max_object_bytes = u64::MAX).await;
    let caps = srv2.client("").get("/v1/capabilities").await;
    assert_eq!(caps.json["limits"]["max_object_bytes"].as_u64(), Some(ag_server::config::engine_max_object_bytes()));
    srv.stop().await;
    srv2.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_and_grpc_refuse_a_request_at_the_same_configured_size() {
    let limit = 100 * 1024u64;
    let srv = TestServer::start_with(move |cfg| cfg.limits.max_request_bytes = limit).await;
    let admin = srv.admin();

    // HTTP: a description of 40 KB is fine, one of 200 KB is too large.
    let small = admin.post("/v1/repos", json!({"name": "acme/small", "description": "x".repeat(40_000)})).await;
    assert_eq!(small.status, 200, "{}", small.json);
    let big = admin.post("/v1/repos", json!({"name": "acme/big", "description": "x".repeat(200_000)})).await;
    assert_eq!(big.status, 413, "{}", big.json);

    // gRPC applied a fixed 16 MiB whatever the configuration said (and Negotiate advertised).
    let ch = Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap();
    let call = |name: &str, description: String| {
        let mut req =
            tonic::Request::new(v1::CreateRepoRequest { name: name.into(), description, ..Default::default() });
        req.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {ADMIN_TOKEN}")).unwrap());
        req
    };
    let mut repos = RepositoryServiceClient::new(ch);
    repos.create(call("acme/small-grpc", "x".repeat(40_000))).await.expect("under the limit");
    let status = repos.create(call("acme/big-grpc", "x".repeat(200_000))).await.unwrap_err();
    assert!(
        matches!(status.code(), tonic::Code::OutOfRange | tonic::Code::ResourceExhausted),
        "refused for its size: {status}"
    );
    assert!(srv.app.control.repo("acme/big-grpc").unwrap().is_none(), "and nothing was created");

    // What Negotiate says is what both transports do.
    let caps = srv.client("").get("/v1/capabilities").await;
    assert_eq!(caps.json["limits"]["max_request_bytes"].as_u64(), Some(limit));
    srv.stop().await;
}
