//! Server hardening that needs no credentials machinery: exact repository counts, and a preparation that is
//! read in one piece while a validation finishes.

mod common;

use std::time::{Duration, Instant};

use ag_crypto::KeyPair;
use ag_policy::{Attestation, CheckResult};
use ag_server::control::RepoRow;
use ag_types::{Identity, Timestamp, TreeId};
use common::*;
use serde_json::{Value, json};

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}")).to_string()
}

// ---- repository listings -------------------------------------------------------------------------------

async fn propose(
    alice: &Http,
    repo: &str,
    name: &str,
    parent: Option<ag_types::ChangesetId>,
) -> (String, ag_types::ChangesetId) {
    let c = Client::new(repo);
    let (head, objs) =
        c.commit(parent.into_iter().collect(), "user:alice", name, &[(name, name.as_bytes())], 1_700_000_000_000);
    upload(alice, repo, &objs).await;
    let p = alice
        .post(
            &format!("/v1/repos/{repo}/proposals"),
            json!({"stream": "main", "head": head.to_string(), "title": name}),
        )
        .await;
    assert_eq!(p.status, 200, "{}", p.json);
    (s(&p.json["id"]), head)
}

fn counts(repo: &Value) -> (u64, u64, String) {
    (
        repo["stream_count"].as_u64().unwrap_or(0),
        repo["changeset_count"].as_u64().unwrap_or(0),
        repo["counts_error"].as_str().unwrap_or("").to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_listings_carry_the_real_number_of_streams_and_changesets() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    let created = admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    assert_eq!(created.status, 200, "{}", created.json);
    // A new repository has its default stream and nothing in it: that is what create says (it used to say 0 / 0).
    assert_eq!(counts(&created.json), (1, 0, String::new()), "{}", created.json);
    admin.post("/v1/repos", json!({"name": "acme/empty"})).await;

    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let (p1, head1) = propose(&alice, "acme/web", "one", None).await;
    let (p2, _) = propose(&alice, "acme/web", "two", Some(head1)).await;
    for p in [&p1, &p2] {
        let r = carol.post(&format!("/v1/repos/acme/web/proposals/{p}/integrate"), json!({})).await;
        assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
    }
    assert_eq!(admin.post("/v1/repos/acme/web/streams", json!({"name": "dev"})).await.status, 200);
    assert_eq!(admin.post("/v1/repos/acme/web/streams", json!({"name": "release"})).await.status, 200);

    let list = admin.get("/v1/repos").await;
    assert_eq!(list.status, 200, "{}", list.json);
    let by_name =
        |name: &str| list.json["repos"].as_array().unwrap().iter().find(|r| r["name"] == name).cloned().unwrap();
    assert_eq!(counts(&by_name("acme/web")), (3, 2, String::new()), "{}", list.json);
    assert_eq!(counts(&by_name("acme/empty")), (1, 0, String::new()));
    // The listing, the description and the creation agree.
    let described = admin.get("/v1/repos/acme/web").await;
    assert_eq!(counts(&described.json), counts(&by_name("acme/web")));
    // A second creation with the same idempotency key answers with what the repository is now, not what it was.
    let again = admin.post("/v1/repos", json!({"name": "acme/keyed", "idempotency_key": "k1"})).await;
    assert_eq!(counts(&again.json), (1, 0, String::new()));
    let replay = admin.post("/v1/repos", json!({"name": "acme/keyed", "idempotency_key": "k1"})).await;
    assert_eq!(counts(&replay.json), (1, 0, String::new()));
    assert_eq!(admin.post("/v1/repos/acme/keyed/streams", json!({"name": "dev"})).await.status, 200);
    let replay = admin.post("/v1/repos", json!({"name": "acme/keyed", "idempotency_key": "k1"})).await;
    assert_eq!(counts(&replay.json), (2, 0, String::new()), "{}", replay.json);

    // A repository that cannot be read does not break the listing, and does not pose as an empty one.
    srv.app
        .control
        .insert_repo(&RepoRow {
            name: "acme/elsewhere".into(),
            namespace: "acme/elsewhere".into(),
            default_stream: "main".into(),
            description: String::new(),
            created_at_ms: 1,
            created_by: "user:root".into(),
            quota: Default::default(),
            object_bytes: 0,
            config_json: String::new(),
        })
        .unwrap();
    let list = admin.get("/v1/repos").await;
    assert_eq!(list.status, 200, "{}", list.json);
    let by_name =
        |name: &str| list.json["repos"].as_array().unwrap().iter().find(|r| r["name"] == name).cloned().unwrap();
    let broken = counts(&by_name("acme/elsewhere"));
    assert_eq!((broken.0, broken.1), (0, 0));
    assert!(!broken.2.is_empty(), "the listing must say why the counts are not there: {}", by_name("acme/elsewhere"));
    assert!(!broken.2.contains(srv.dir.path().to_str().unwrap()), "and not with a path of the server: {}", broken.2);
    assert_eq!(counts(&by_name("acme/web")), (3, 2, String::new()), "the others are unaffected");
    srv.stop().await;
}

// ---- integration.prepare against a validation that finishes at that moment ---------------------------

fn policy_for(issuer: &str, key: &KeyPair) -> String {
    json!({
        "trusted_issuers": [{"identity": issuer, "public_key": key.public().to_hex()}],
        "required_validations": [{"check": "tests", "run": {"argv": ["cargo", "test"], "timeout_secs": 120, "memory_mb": 512}}]
    })
    .to_string()
}

fn attest(key: &KeyPair, issuer: &str, job: &Value) -> Value {
    let v = &job["validation"];
    let a = Attestation::sign(
        key,
        format!("att_{}", job["id"].as_str().unwrap()),
        v["tree_id"].as_str().unwrap().parse::<TreeId>().unwrap(),
        v["check"].as_str().unwrap().to_string(),
        CheckResult::Pass,
        v["policy_version"].as_u64().unwrap(),
        "env-test".into(),
        Identity::new(issuer).unwrap(),
        Timestamp::from_millis(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
        ),
    )
    .unwrap();
    json!({
        "id": a.id, "tree_id": a.tree_id.to_string(), "check": a.check, "result": a.result.as_str(),
        "policy_version": a.policy_version, "environment_digest": a.environment_digest, "issuer": a.issuer.to_string(),
        "issued_at_ms": a.issued_at.as_millis(), "public_key": a.public_key.to_hex(), "signature": a.signature.to_hex()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_preparation_is_one_reading_even_when_a_validation_finishes_while_it_is_made() {
    // Two engine handles (and so two database connections) per repository: the preparation holds its read
    // transaction open on one while the worker's completion commits through the other.
    let srv = TestServer::start_with(|cfg| cfg.repos.pool_size = 2).await;
    a_preparation_is_one_reading(srv).await;
}

/// The same with the repository's metadata in PostgreSQL, where a read transaction is `REPEATABLE READ`. Runs when
/// `AG_TEST_POSTGRES_URL` is set (see `scripts/pg-test.sh`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_preparation_is_one_reading_on_postgres_too() {
    let Some(url) = ag_testkit::require_here(
        "postgres",
        "AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)",
        ag_storage::testing::postgres_url(),
    ) else {
        return;
    };
    let schema = ag_storage::testing::unique("agprep");
    let template = ag_integrate::StorageConfig {
        meta: ag_integrate::MetaConfig::Postgres {
            url_env: Some("AG_TEST_POSTGRES_URL".into()),
            url: None,
            schema: schema.clone(),
            pool_size: Some(4),
        },
        objects: ag_integrate::ObjectsConfig::Fs,
    };
    let srv = TestServer::start_with(move |cfg| {
        cfg.repos.pool_size = 2;
        cfg.repos.storage = Some(template);
    })
    .await;
    a_preparation_is_one_reading(srv).await;
    let name = format!("{schema}_acme_web");
    let _ = tokio::task::spawn_blocking(move || ag_storage::meta::drop_schema(&url, &name)).await;
}

async fn a_preparation_is_one_reading(srv: TestServer) {
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let ci_key = KeyPair::generate().unwrap();
    let r = admin
        .call(
            "PUT",
            "/v1/repos/acme/web/policy",
            Some(json!({"stream": "main", "document": policy_for("service:ci-1", &ci_key)})),
            &[],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    let (pid, _) = propose(&alice, "acme/web", "first", None).await;

    // The validation is queued, and a worker has claimed it: running.
    let prep =
        alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({"enqueue_validations": true})).await;
    assert_eq!(prep.json["validations"][0]["state"], "pending", "{}", prep.json);
    let job_id = s(&prep.json["jobs"][0]["id"]);
    let claim = ci.post("/v1/jobs/claim", json!({"worker": "w1", "lease_ms": 60000, "wait_ms": 2000})).await;
    let job = claim.json["job"].clone();
    assert_eq!(s(&job["id"]), job_id);

    // Hold the next preparation inside its snapshot - after the attestations were read, before the jobs are -
    // and let the worker finish the job right there.
    let gates: Vec<_> = (0..2)
        .map(|_| srv.app.repos.handle("acme/web").unwrap().failpoints().arm_gate("prepare.in_snapshot"))
        .collect();
    let preparing = {
        let (alice, path) = (alice.clone(), format!("/v1/repos/acme/web/proposals/{pid}/prepare"));
        tokio::spawn(async move { alice.post(&path, json!({})).await })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let held = loop {
        if let Some(i) = gates.iter().position(|g| g.wait_reached(Duration::from_millis(20))) {
            break i;
        }
        assert!(Instant::now() < deadline, "the preparation never reached its snapshot");
    };
    let att = attest(&ci_key, "service:ci-1", &job);
    let done = ci
        .post(&format!("/v1/repos/acme/web/jobs/{job_id}/complete"), json!({"worker": "w1", "attestation": att}))
        .await;
    assert_eq!(done.status, 200, "the worker finishes while the preparation is in flight: {}", done.json);
    assert_eq!(done.json["state"], "succeeded");
    for (i, gate) in gates.into_iter().enumerate() {
        if i == held {
            gate.release();
        }
    }
    let during = preparing.await.unwrap();
    assert_eq!(during.status, 200, "{}", during.json);

    // What it answers is what the repository looked like at one moment: the verdict and the job agree. Before,
    // the verdict was read first and the jobs after - "missing" next to a job that was already done, i.e. a check
    // that nobody is running and nothing has passed.
    let (state, job_state) = (during.json["validations"][0]["state"].clone(), during.json["jobs"][0]["state"].clone());
    match (state.as_str().unwrap(), job_state.as_str().unwrap()) {
        ("pending", "claimed") | ("passed", "succeeded") => {}
        other => panic!("a preparation that contradicts itself: {other:?}: {}", during.json),
    }
    assert_eq!(during.json["validations"][0]["job_id"], job_id.as_str(), "{}", during.json);

    // Afterwards the world is plain: passed, and the job is done.
    let after = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({})).await;
    assert_eq!(after.json["validations"][0]["state"], "passed", "{}", after.json);
    assert_eq!(after.json["jobs"][0]["state"], "succeeded");
    srv.stop().await;
}
