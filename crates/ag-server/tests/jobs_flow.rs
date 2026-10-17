//! The execution plane through the API: queued validations, workers, signed attestations.

mod common;

use ag_crypto::KeyPair;
use ag_policy::{Attestation, CheckResult};
use ag_types::{Identity, Timestamp, TreeId};
use common::*;
use serde_json::{Value, json};

fn policy_for(issuer: &str, key: &KeyPair) -> String {
    json!({
        "trusted_issuers": [{"identity": issuer, "public_key": key.public().to_hex()}],
        "required_validations": [{"check": "tests", "run": {"argv": ["cargo", "test"], "timeout_secs": 120, "memory_mb": 512}}]
    })
    .to_string()
}

fn attest(key: &KeyPair, issuer: &str, job: &Value, result: CheckResult) -> Value {
    let v = &job["validation"];
    let a = Attestation::sign(
        key,
        format!("att_{}", job["id"].as_str().unwrap()),
        v["tree_id"].as_str().unwrap().parse::<TreeId>().unwrap(),
        v["check"].as_str().unwrap().to_string(),
        result,
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

async fn seed(srv: &TestServer, repo: &str, issuer_key: &KeyPair) -> (Http, Http, String) {
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": repo})).await;
    let r = admin
        .call(
            "PUT",
            &format!("/v1/repos/{repo}/policy"),
            Some(json!({"stream": "main", "document": policy_for("service:ci-1", issuer_key)})),
            &[],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let alice = user_with_role(srv, "user:alice", repo, "writer").await;
    let carol = user_with_role(srv, "user:carol", repo, "integrator").await;
    let c = Client::new(repo);
    let (head, objs) = c.commit(vec![], "user:alice", "first", &[("lib.rs", b"fn main() {}\n")], 1_700_000_000_000);
    upload(&alice, repo, &objs).await;
    let p = alice
        .post(
            &format!("/v1/repos/{repo}/proposals"),
            json!({"stream": "main", "head": head.to_string(), "title": "first"}),
        )
        .await;
    (alice, carol, p.json["id"].as_str().unwrap().to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_validates_the_exact_candidate_and_unblocks_the_integration() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;

    // Blocked on a missing validation; the preparation says so and queues the job when asked.
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "blocked", "{}", r.json);
    let prep =
        alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({"enqueue_validations": true})).await;
    assert_eq!(prep.status, 200, "{}", prep.json);
    assert_eq!(prep.json["validations"][0]["state"], "pending", "{}", prep.json);
    let job_id = prep.json["jobs"][0]["id"].as_str().unwrap().to_string();
    // Asking again does not queue a second job for the same candidate.
    let again = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    assert_eq!(again.json["jobs"][0]["id"], job_id.as_str());

    // Only runners can claim.
    assert_eq!(alice.post("/v1/jobs/claim", json!({"repo": "acme/web"})).await.status, 403);

    // The worker claims (long-poll returns immediately), checks out the candidate, and signs.
    let claim = ci.post("/v1/jobs/claim", json!({"worker": "w1", "lease_ms": 60000, "wait_ms": 2000})).await;
    assert_eq!(claim.status, 200, "{}", claim.json);
    let job = claim.json["job"].clone();
    assert_eq!(job["id"], job_id.as_str());
    assert_eq!(job["state"], "claimed");
    assert_eq!(job["validation"]["run"]["argv"][0], "cargo");
    assert_eq!(job["validation"]["run"]["network"], false, "network is off unless the policy says so");
    let tree = job["validation"]["tree_id"].as_str().unwrap().to_string();
    let cand = ci.get(&format!("/v1/repos/acme/web/proposals/{pid}/candidate?expected_tree_id={tree}")).await;
    assert_eq!(cand.status, 200, "{}", cand.json);
    assert_eq!(cand.json["tree_id"], tree.as_str());
    assert_eq!(cand.json["entries"][0]["path"], "lib.rs");
    // A worker that asks for the wrong tree is told the candidate is stale.
    let stale =
        ci.get(&format!("/v1/repos/acme/web/proposals/{pid}/candidate?expected_tree_id={}", "ab".repeat(32))).await;
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json["error"]["code"], "stale_candidate");

    // Nobody else can finish this job.
    let other = user_with_role(&srv, "service:ci-2", "acme/web", "runner").await;
    let att = attest(&ci_key, "service:ci-1", &job, CheckResult::Pass);
    let r = other
        .post(&format!("/v1/repos/acme/web/jobs/{job_id}/complete"), json!({"worker": "w1", "attestation": att}))
        .await;
    assert_eq!(r.status, 412, "{}", r.json);
    // An attestation for another tree, check or issuer is refused.
    let mut wrong_tree = att.clone();
    wrong_tree["tree_id"] = json!("cd".repeat(32));
    let r = ci
        .post(&format!("/v1/repos/acme/web/jobs/{job_id}/complete"), json!({"worker": "w1", "attestation": wrong_tree}))
        .await;
    assert_eq!(r.status, 400, "{}", r.json);
    let impostor = attest(&ci_key, "service:ci-2", &job, CheckResult::Pass);
    let r = ci
        .post(&format!("/v1/repos/acme/web/jobs/{job_id}/complete"), json!({"worker": "w1", "attestation": impostor}))
        .await;
    assert_eq!(r.status, 403, "{}", r.json);
    let r = ci.post(&format!("/v1/repos/acme/web/jobs/{job_id}/complete"), json!({"worker": "w1"})).await;
    assert_eq!(r.status, 400, "completion without an attestation is refused");

    // Heartbeat, then complete with the right attestation.
    assert_eq!(
        ci.post(&format!("/v1/repos/acme/web/jobs/{job_id}/heartbeat"), json!({"worker": "w1", "lease_ms": 60000}))
            .await
            .status,
        200
    );
    let r = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{job_id}/complete"),
            json!({"worker": "w1", "attestation": att, "result_json": {"exit": 0}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["state"], "succeeded");

    // The same candidate is now validated by a trusted issuer: the integration goes through.
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trust_comes_from_the_policy_not_from_having_a_job() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    // A runner whose key the policy does not list.
    let rogue = user_with_role(&srv, "service:rogue", "acme/web", "runner").await;
    alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    let claim = rogue.post("/v1/jobs/claim", json!({"worker": "r", "wait_ms": 1000})).await;
    let job = claim.json["job"].clone();
    let rogue_key = KeyPair::generate().unwrap();
    let att = attest(&rogue_key, "service:rogue", &job, CheckResult::Pass);
    let r = rogue
        .post(
            &format!("/v1/repos/acme/web/jobs/{}/complete", job["id"].as_str().unwrap()),
            json!({"worker": "r", "attestation": att}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    // The attestation is stored but does not count.
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "blocked", "{}", r.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_leases_requeue_and_failures_can_retry() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, _carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;

    // A worker claims with a tiny lease and then vanishes: the job comes back.
    let first = ci.post("/v1/jobs/claim", json!({"worker": "crashy", "lease_ms": 50})).await;
    assert!(first.json["job"].is_object());
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let nothing_for_others = ci.post("/v1/jobs/claim", json!({"worker": "healthy", "lease_ms": 60000})).await;
    let job = nothing_for_others.json["job"].clone();
    assert!(job.is_object(), "the abandoned job is handed out again: {}", nothing_for_others.json);
    assert_eq!(job["attempts"], 2);
    // The first worker has lost its claim and learns so.
    let jid = job["id"].as_str().unwrap();
    let r = ci.post(&format!("/v1/repos/acme/web/jobs/{jid}/heartbeat"), json!({"worker": "crashy"})).await;
    assert_ne!(r.status, 200);

    // A retryable failure puts the job back after the delay; a permanent one ends it.
    let r = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{jid}/fail"),
            json!({"worker": "healthy", "error": "sandbox unavailable", "retry_after_ms": 1}),
        )
        .await;
    assert_eq!(r.json["state"], "queued", "{}", r.json);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let again = ci.post("/v1/jobs/claim", json!({"worker": "healthy", "lease_ms": 60000})).await;
    assert_eq!(again.json["job"]["attempts"], 3);
    let r = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{jid}/fail"),
            json!({"worker": "healthy", "error": "the command does not exist"}),
        )
        .await;
    assert_eq!(r.json["state"], "failed");
    let jobs = alice.get("/v1/repos/acme/web/jobs?state=failed").await;
    assert_eq!(jobs.json["jobs"].as_array().unwrap().len(), 1);
    // The same story as counters.
    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    for (event, n) in [("enqueued", 1), ("claimed", 3), ("retried", 1), ("failed", 1)] {
        assert!(m.contains(&format!("agwp_job_events_total{{event=\"{event}\"}} {n}")), "{event}: {m}");
    }
    assert!(m.contains("agwp_jobs_queued 0"), "{m}");
    // The job life cycle is part of the audited event feed.
    let ev = alice.get("/v1/repos/acme/web/events/replay?topics=job&limit=100").await;
    let topics: Vec<&str> =
        ev.json["events"].as_array().unwrap().iter().map(|e| e["topic"].as_str().unwrap()).collect();
    assert!(
        topics.contains(&"job.enqueue") && topics.contains(&"job.claim") && topics.contains(&"job.fail"),
        "{topics:?}"
    );
    srv.stop().await;
}
