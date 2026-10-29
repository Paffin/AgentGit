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
    // "retried" counts the lease that ran out as well as the explicit retry.
    for (event, n) in [("enqueued", 1), ("claimed", 3), ("retried", 2), ("failed", 1)] {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runner_that_dies_on_its_last_attempt_is_counted_and_audited() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, _carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;

    // The runner claims the job with a tiny lease and crashes, again and again, until the attempts are used up.
    // Nothing ever calls `fail`: the server itself has to notice.
    let mut claims = 0;
    loop {
        let r = ci.post("/v1/jobs/claim", json!({"worker": "crashy", "lease_ms": 20})).await;
        if r.json["job"].is_object() {
            claims += 1;
            assert!(claims <= 20, "the job is handed out for ever");
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        } else {
            break;
        }
    }
    let jobs = alice.get("/v1/repos/acme/web/jobs?state=failed").await;
    let failed = jobs.json["jobs"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{}", jobs.json);
    assert_eq!(failed[0]["attempts"], claims);
    assert!(failed[0]["error"].as_str().unwrap().contains("lease expired"), "{}", failed[0]);

    // The failure is visible where operators look for it: the counter behind the AgentGitJobsFailing alert ...
    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    assert!(m.contains("agwp_job_events_total{event=\"failed\"} 1"), "{m}");
    assert!(m.contains(&format!("agwp_job_events_total{{event=\"retried\"}} {}", claims - 1)), "{m}");
    // ... and the audit feed, with a record of every time a job was taken back.
    let ev = alice.get("/v1/repos/acme/web/events/replay?topics=job.expire&limit=100").await;
    let states: Vec<&str> = ev.json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["payload"]["details"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states.len() as i64, claims, "{states:?}");
    assert_eq!(states.last(), Some(&"failed"));
    assert!(states[..states.len() - 1].iter().all(|s| *s == "queued"));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_are_deduplicated_by_key_however_many_jobs_came_later_and_errors_are_bounded() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, _carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    let first = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    let job_id = first.json["jobs"][0]["id"].as_str().unwrap().to_string();

    // 1100 jobs enqueued later push the first one out of the 1000 newest, the window the old code searched.
    let handle = srv.app.repos.handle("acme/web").unwrap();
    handle
        .meta()
        .write(|tx| {
            for i in 0..1100 {
                tx.job_enqueue("other", &json!({"i": i}), Some(&format!("other:{i}")), Timestamp::from_millis(0))?;
            }
            Ok(())
        })
        .unwrap();
    let audit_before = handle.meta().read(|tx| tx.audit_latest_seq()).unwrap();
    let again = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    assert_eq!(again.json["jobs"][0]["id"], job_id.as_str(), "the same job, not another: {}", again.json);
    assert_eq!(
        handle.meta().read(|tx| tx.audit_latest_seq()).unwrap(),
        audit_before,
        "nothing was enqueued or audited again"
    );
    let prep =
        alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({"enqueue_validations": true})).await;
    assert_eq!(prep.status, 200, "{}", prep.json);
    let queued = srv.client(ADMIN_TOKEN).get("/metrics").await;
    let m = String::from_utf8(queued.body).unwrap();
    assert!(m.contains("agwp_job_events_total{event=\"enqueued\"} 1"), "{m}");

    // An error text of any size is stored - and audited - at the same bounded length.
    let claim = ci.post("/v1/jobs/claim", json!({"worker": "w", "kinds": ["validate"], "wait_ms": 1000})).await;
    assert_eq!(claim.json["job"]["id"], job_id.as_str(), "{}", claim.json);
    let huge = "e".repeat(500_000);
    let r = ci.post(&format!("/v1/repos/acme/web/jobs/{job_id}/fail"), json!({"worker": "w", "error": huge})).await;
    assert_eq!(r.json["state"], "failed", "{}", r.json);
    let stored = r.json["error"].as_str().unwrap().len();
    assert_eq!(stored, 2000);
    let ev = alice.get("/v1/repos/acme/web/events/replay?topics=job.fail&limit=10").await;
    let audited = ev.json["events"][0]["payload"]["details"]["error"].as_str().unwrap().len();
    assert_eq!(audited, stored, "the audit entry is bounded like the job");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asking_again_finds_the_queued_validation_however_many_jobs_came_after_it() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, _carol, pid) = seed(&srv, "acme/web", &ci_key).await;

    let prep =
        alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({"enqueue_validations": true})).await;
    assert_eq!(prep.status, 200, "{}", prep.json);
    let job_id = prep.json["jobs"][0]["id"].as_str().unwrap().to_string();
    async fn enqueue_events(alice: &Http) -> usize {
        let ev = alice.get("/v1/repos/acme/web/events/replay?topics=job.enqueue&limit=1000").await;
        ev.json["events"].as_array().unwrap().len()
    }
    assert_eq!(enqueue_events(&alice).await, 1);

    // More jobs than the first page of the job list holds are queued after it (by other work, straight into the
    // repository's queue).
    let repo = ag_integrate::Repository::open(&srv.dir.path().join("repos/acme/web")).unwrap();
    for i in 0..1100 {
        let key = format!("noise:{i}");
        repo.meta()
            .write(|tx| tx.job_enqueue("unrelated", &json!({"n": i}), Some(&key), Timestamp::from_millis(0)))
            .unwrap();
    }

    // The same candidate is requested again: the existing job is found by its key, not by scanning the newest
    // thousand, so nothing is enqueued, counted or audited a second time.
    let again = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    assert_eq!(again.status, 200, "{}", again.json);
    assert_eq!(again.json["jobs"].as_array().unwrap().len(), 1, "{}", again.json);
    assert_eq!(again.json["jobs"][0]["id"], job_id.as_str());
    assert_eq!(enqueue_events(&alice).await, 1, "no second job.enqueue audit entry for work that was not enqueued");
    let m = String::from_utf8(srv.client("").get("/metrics").await.body).unwrap();
    assert!(m.contains("agwp_job_events_total{event=\"enqueued\"} 1"), "{m}");

    // The read-only view of the candidate's jobs sees it too, so the check still reads as pending, not missing.
    let prep = alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/prepare"), json!({})).await;
    assert_eq!(prep.json["jobs"].as_array().map_or(0, Vec::len), 1, "{}", prep.json);
    assert_eq!(prep.json["validations"][0]["state"], "pending", "{}", prep.json);
    assert_eq!(prep.json["validations"][0]["job_id"], job_id.as_str());
    srv.stop().await;
}

/// The same worker name claims, loses its lease, and claims the job again: the zombie that still holds the first
/// claim's number cannot touch the second claim, though its name is the same; a request without a number is checked
/// as before (by name).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_claim_of_the_same_worker_is_fenced_off() {
    let srv = TestServer::start().await;
    let ci_key = KeyPair::generate().unwrap();
    let (alice, _carol, pid) = seed(&srv, "acme/web", &ci_key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;

    let first = ci.post("/v1/jobs/claim", json!({"worker": "w", "lease_ms": 50})).await.json["job"].clone();
    let (jid, first_claim) = (first["id"].as_str().unwrap().to_string(), first["attempts"].as_u64().unwrap());
    assert_eq!(first_claim, 1);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let second = ci.post("/v1/jobs/claim", json!({"worker": "w", "lease_ms": 60000})).await.json["job"].clone();
    assert_eq!(second["id"], first["id"], "the same job, claimed again by the same worker name");
    let second_claim = second["attempts"].as_u64().unwrap();
    assert_eq!(second_claim, 2);

    let path = |what: &str| format!("/v1/repos/acme/web/jobs/{jid}/{what}");
    // The old claim's number is turned away on every route; the job is untouched.
    let beat = ci.post(&path("heartbeat"), json!({"worker": "w", "fencing_token": first_claim})).await;
    assert_ne!(beat.status, 200, "{}", beat.json);
    let fail = ci.post(&path("fail"), json!({"worker": "w", "fencing_token": first_claim, "error": "late"})).await;
    assert_ne!(fail.status, 200, "{}", fail.json);
    let done = ci
        .post(
            &path("complete"),
            json!({"worker": "w", "fencing_token": first_claim, "stale": true, "result_json": "{}"}),
        )
        .await;
    assert_eq!(done.status, 412, "{}", done.json);
    assert_eq!(done.json["error"]["code"], "failed_precondition", "{}", done.json);
    let now = alice.get(&format!("/v1/repos/acme/web/jobs/{jid}")).await.json;
    assert_eq!((now["state"].as_str(), now["attempts"].as_u64()), (Some("claimed"), Some(2)), "{now}");

    // The current claim's number works; so does a request without one (the identity check of before).
    let beat = ci.post(&path("heartbeat"), json!({"worker": "w", "fencing_token": second_claim})).await;
    assert_eq!(beat.status, 200, "{}", beat.json);
    let beat = ci.post(&path("heartbeat"), json!({"worker": "w"})).await;
    assert_eq!(beat.status, 200, "{}", beat.json);
    // A number that cannot be a claim of any job is refused as invalid, not read as "no number".
    let huge = ci.post(&path("heartbeat"), json!({"worker": "w", "fencing_token": 1u64 << 40})).await;
    assert_eq!(huge.status, 400, "{}", huge.json);
    let done = ci
        .post(
            &path("complete"),
            json!({"worker": "w", "fencing_token": second_claim, "stale": true, "result_json": "{}"}),
        )
        .await;
    assert_eq!(done.status, 200, "{}", done.json);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strict_job_fencing_requires_the_claim_on_every_mutation() {
    let srv = TestServer::start_with(|cfg| {
        // Parse the operator-facing configuration, including its strict-mode spelling.
        let mut value = toml::Value::try_from(&*cfg).unwrap();
        value
            .as_table_mut()
            .unwrap()
            .insert("jobs".into(), toml::from_str::<toml::Value>("require_fencing_token = true").unwrap());
        *cfg = value.try_into().unwrap();
    })
    .await;
    let key = KeyPair::generate().unwrap();
    let (alice, _, pid) = seed(&srv, "acme/web", &key).await;
    let ci = user_with_role(&srv, "service:ci-1", "acme/web", "runner").await;
    alice.post(&format!("/v1/repos/acme/web/proposals/{pid}/validate"), json!({})).await;
    let job = ci.post("/v1/jobs/claim", json!({"worker":"w"})).await.json["job"].clone();
    let jid = job["id"].as_str().unwrap();
    let token = job["attempts"].as_u64().unwrap();
    for action in ["heartbeat", "fail", "complete"] {
        let path = format!("/v1/repos/acme/web/jobs/{jid}/{action}");
        for omitted in [true, false] {
            let mut body = json!({"worker":"w", "stale":true});
            if !omitted {
                body["fencing_token"] = json!(0);
            }
            let r = ci.post(&path, body).await;
            assert_eq!(r.status, 412, "{action}: {}", r.json);
            assert!(r.json["error"]["message"].as_str().unwrap().contains("fencing_token"));
        }
    }
    let current = alice.get(&format!("/v1/repos/acme/web/jobs/{jid}")).await.json;
    assert_eq!(current, job, "refused calls must leave the claim untouched");
    let beat = ci
        .post(&format!("/v1/repos/acme/web/jobs/{jid}/heartbeat"), json!({"worker":"w", "fencing_token":token}))
        .await;
    assert_eq!(beat.status, 200, "{}", beat.json);
    let fail = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{jid}/fail"),
            json!({"worker":"w", "fencing_token":token, "retry_after_ms":1}),
        )
        .await;
    assert_eq!(fail.status, 200, "{}", fail.json);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let next = ci.post("/v1/jobs/claim", json!({"worker":"w"})).await.json["job"].clone();
    assert_eq!(next["id"], job["id"]);
    assert!(next["attempts"].as_u64().unwrap() > token);
    let stale = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{jid}/complete"),
            json!({"worker":"w", "fencing_token":token, "stale":true}),
        )
        .await;
    assert_eq!(stale.status, 412);
    let done = ci
        .post(
            &format!("/v1/repos/acme/web/jobs/{jid}/complete"),
            json!({"worker":"w", "fencing_token":next["attempts"], "stale":true}),
        )
        .await;
    assert_eq!(done.status, 200, "{}", done.json);
    assert_eq!(done.json["state"], "succeeded");
    srv.stop().await;
}
