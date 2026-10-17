//! Agents through the API: identities, runs, leases, virtual workspaces, signed provenance.

mod common;

use ag_agent::provenance::ProvenanceRecord;
use ag_crypto::KeyPair;
use ag_types::{ChangesetId, Identity, Timestamp};
use base64::Engine as _;
use common::*;
use serde_json::{Value, json};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

async fn agent_token(srv: &TestServer, name: &str, repo: &str) -> Http {
    let admin = srv.admin();
    let subject = format!("agent:{name}");
    assert_eq!(admin.post("/v1/roles", json!({"subject": subject, "repo": repo, "role": "writer"})).await.status, 200);
    let r =
        admin.post("/v1/credentials", json!({"subject": subject, "ttl_secs": 900, "scope": {"repos": [repo]}})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    srv.client(r.json["token"].as_str().unwrap())
}

fn rid(v: &Value) -> String {
    v.as_str().unwrap().to_string()
}

#[tokio::test]
async fn an_agent_edits_a_virtual_workspace_and_lands_signed_work_under_policy() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let bot = agent_token(&srv, "refactor-bot", "acme/web").await;
    let bob = user_with_role(&srv, "user:bob", "acme/web", "reviewer").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;

    // Seed the stream with a human change.
    let c = Client::new("acme/web");
    let (seed, objs) =
        c.commit(vec![], "user:alice", "seed", &[("src/lib.rs", b"fn a() {}\nfn b() {}\n")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objs).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": seed.to_string(), "title": "seed"}))
        .await;
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{}/integrate", rid(&p.json["id"])), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);

    // Policy: agents need signed provenance and one approval.
    let policy = r#"{"require_provenance_for_agents": true, "approvals": {"min": 0, "min_for_agents": 1}}"#;
    assert_eq!(
        admin
            .call("PUT", "/v1/repos/acme/web/policy", Some(json!({"stream": "main", "document": policy})), &[])
            .await
            .status,
        200
    );

    // The agent registers its public key and starts a run.
    let key = KeyPair::generate().unwrap();
    let r = bot
        .post("/v1/repos/acme/web/agents", json!({"name": "refactor-bot", "public_key": key.public().to_hex()}))
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    // It may not register someone else's name.
    let r =
        bot.post("/v1/repos/acme/web/agents", json!({"name": "other-bot", "public_key": key.public().to_hex()})).await;
    assert_eq!(r.status, 403, "{}", r.json);
    let run = bot.post("/v1/repos/acme/web/runs", json!({"agent": "refactor-bot"})).await;
    assert_eq!(run.status, 200, "{}", run.json);
    let run_id = rid(&run.json["run_id"]);
    assert_eq!(run.json["identity"], format!("agent:refactor-bot#{run_id}"));
    let as_run = [("ag-run", run_id.as_str())];

    // A virtual workspace: no checkout, just API calls.
    let r = bot
        .call("POST", "/v1/repos/acme/web/workspaces", Some(json!({"name": "ws-1", "stream": "main"})), &as_run)
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["head"], seed.to_string());
    let r = bot
        .call(
            "POST",
            "/v1/repos/acme/web/workspaces/ws-1/changes",
            Some(json!({"message": "rename a to alpha", "changes": [{"path": "src/lib.rs", "op": "put", "content": b64("fn alpha() {}\nfn b() {}\n")}, {"path": "NOTES.md", "op": "put", "content": b64("notes\n")}]})),
            &as_run,
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let ws_head = rid(&r.json["head"]);
    // An optimistic-concurrency mismatch is refused.
    let stale = bot
        .call("POST", "/v1/repos/acme/web/workspaces/ws-1/changes", Some(json!({"expected_head": seed.to_string(), "changes": [{"path": "x", "op": "put", "content": b64("x")}]})), &as_run)
        .await;
    assert_eq!(stale.status, 412, "{}", stale.json);
    // Someone else cannot touch the agent's workspace.
    let r = alice
        .post(
            "/v1/repos/acme/web/workspaces/ws-1/changes",
            json!({"changes": [{"path": "x", "op": "put", "content": b64("x")}]}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.json);
    // The workspace content is readable through the tree API at its head.
    let f = bot.get(&format!("/v1/repos/acme/web/files/src/lib.rs?rev={ws_head}")).await;
    assert_eq!(f.body, b"fn alpha() {}\nfn b() {}\n");

    // Propose it as the run.
    let prop = bot
        .call(
            "POST",
            "/v1/repos/acme/web/proposals",
            Some(json!({"stream": "main", "head": ws_head, "title": "Rename a to alpha"})),
            &as_run,
        )
        .await;
    assert_eq!(prop.status, 200, "{}", prop.json);
    assert_eq!(prop.json["author"], format!("agent:refactor-bot#{run_id}"));
    let pid = rid(&prop.json["id"]);

    // The agent cannot integrate on its own (policy), and without provenance even a human is blocked.
    let r = bot.call("POST", &format!("/v1/repos/acme/web/proposals/{pid}/integrate"), Some(json!({})), &as_run).await;
    assert_eq!(r.status, 403, "agents hold no integrator role: {}", r.json);
    bob.post(&format!("/v1/repos/acme/web/proposals/{pid}/reviews"), json!({"decision": "approve"})).await;
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "blocked", "{}", r.json);
    assert!(r.json["outcome"]["blockers"].to_string().contains("provenance"), "{}", r.json);

    // The agent signs its provenance with its own key; a forged signature is refused.
    let mut rec = ProvenanceRecord::new(
        ws_head.parse::<ChangesetId>().unwrap(),
        Identity::new(format!("agent:refactor-bot#{run_id}")).unwrap(),
        Timestamp::from_millis(1_700_000_100_000),
    );
    rec.run_id = Some(run_id.clone());
    rec.model = Some("example-model".into());
    rec.rationale = Some("rename for clarity".into());
    let wire = json!({
        "changeset": ws_head, "agent": format!("agent:refactor-bot#{run_id}"), "run_id": run_id,
        "model": "example-model", "rationale": "rename for clarity", "created_at_ms": 1_700_000_100_000i64
    });
    let sig = key.sign("provenance.v1", &rec.canonical_bytes().unwrap()).unwrap().to_hex();
    let forger = KeyPair::generate().unwrap();
    let bad_sig = forger.sign("provenance.v1", &rec.canonical_bytes().unwrap()).unwrap().to_hex();
    let r = bot
        .call("POST", "/v1/repos/acme/web/provenance", Some(json!({"record": wire, "signature": bad_sig})), &as_run)
        .await;
    assert_eq!(r.status, 400, "{}", r.json);
    let r = bot
        .call("POST", "/v1/repos/acme/web/provenance", Some(json!({"record": wire, "signature": sig})), &as_run)
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["verified"], true);
    let listed = alice.get(&format!("/v1/repos/acme/web/provenance?changeset={ws_head}")).await;
    assert_eq!(listed.json["entries"][0]["verified"], true);

    // Now the integrator can land it.
    let r = carol.post(&format!("/v1/repos/acme/web/proposals/{pid}/integrate"), json!({})).await;
    assert_eq!(r.json["outcome"]["status"], "integrated", "{}", r.json);
    let f = alice.get("/v1/repos/acme/web/files/src/lib.rs?rev=main").await;
    assert_eq!(f.body, b"fn alpha() {}\nfn b() {}\n");

    // Finish the run; heartbeats and completion are only for the run's own agent.
    let hb = bot.post(&format!("/v1/repos/acme/web/runs/{run_id}/heartbeat"), json!({})).await;
    assert_eq!(hb.status, 200, "{}", hb.json);
    let done = bot
        .post(
            &format!("/v1/repos/acme/web/runs/{run_id}/complete"),
            json!({"status": "completed", "summary": "renamed"}),
        )
        .await;
    assert_eq!(done.json["status"], "completed", "{}", done.json);
    // A finished run can no longer be used as an identity.
    let r = bot
        .call("POST", "/v1/repos/acme/web/workspaces", Some(json!({"name": "ws-2", "stream": "main"})), &as_run)
        .await;
    assert_eq!(r.status, 412, "{}", r.json);
    srv.stop().await;
}

#[tokio::test]
async fn leases_tasks_and_conflicting_workspaces() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let a1 = agent_token(&srv, "worker-a", "acme/web").await;
    let a2 = agent_token(&srv, "worker-b", "acme/web").await;
    let carol = user_with_role(&srv, "user:carol", "acme/web", "integrator").await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "writer").await;

    // Leases: soft leases coexist and report overlaps; exclusive ones refuse, with increasing fencing tokens.
    let l1 = a1.post("/v1/repos/acme/web/leases", json!({"scope": "src/**", "exclusive": true, "ttl_ms": 60000})).await;
    assert_eq!(l1.status, 200, "{}", l1.json);
    let t1 = l1.json["lease"]["fencing_token"].as_u64().unwrap();
    let r =
        a2.post("/v1/repos/acme/web/leases", json!({"scope": "src/auth/**", "exclusive": true, "ttl_ms": 60000})).await;
    assert_eq!(r.status, 409, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "lease_conflict");
    let soft = a2.post("/v1/repos/acme/web/leases", json!({"scope": "src/auth/**", "ttl_ms": 60000})).await;
    assert_eq!(soft.status, 200);
    assert_eq!(soft.json["overlapping"].as_array().unwrap().len(), 1);
    let lid = rid(&l1.json["lease"]["id"]);
    assert_eq!(
        a2.call("DELETE", &format!("/v1/repos/acme/web/leases/{lid}"), None, &[]).await.status,
        412,
        "only the holder releases"
    );
    assert_eq!(a1.call("DELETE", &format!("/v1/repos/acme/web/leases/{lid}"), None, &[]).await.status, 200);
    let l2 = a2.post("/v1/repos/acme/web/leases", json!({"scope": "src/**", "exclusive": true, "ttl_ms": 60000})).await;
    assert!(l2.json["lease"]["fencing_token"].as_u64().unwrap() > t1, "fencing tokens only ever increase");

    // Tasks and dependencies.
    let t_a = alice.post("/v1/repos/acme/web/tasks", json!({"title": "extract module"})).await;
    let t_b = alice
        .post("/v1/repos/acme/web/tasks", json!({"title": "use module", "depends_on": [rid(&t_a.json["id"])]}))
        .await;
    assert_eq!(t_b.json["ready"], false);
    let ready = alice.get("/v1/repos/acme/web/tasks?ready_only=true").await;
    assert_eq!(ready.json["tasks"].as_array().unwrap().len(), 1);
    let cyc = alice
        .post(
            &format!("/v1/repos/acme/web/tasks/{}/dependencies", rid(&t_a.json["id"])),
            json!({"depends_on": rid(&t_b.json["id"])}),
        )
        .await;
    assert_eq!(cyc.status, 412, "cycles are refused: {}", cyc.json);
    alice.post(&format!("/v1/repos/acme/web/tasks/{}/finish", rid(&t_a.json["id"])), json!({"state": "done"})).await;
    let b = alice.get(&format!("/v1/repos/acme/web/tasks/{}", rid(&t_b.json["id"]))).await;
    assert_eq!(b.json["ready"], true);

    // Seed, then two workspaces edit the same line.
    let c = Client::new("acme/web");
    let (seed, objs) = c.commit(vec![], "user:alice", "seed", &[("f.txt", b"one\ntwo\nthree\n")], 1_700_000_000_000);
    upload(&alice, "acme/web", &objs).await;
    let p = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": seed.to_string(), "title": "seed"}))
        .await;
    carol.post(&format!("/v1/repos/acme/web/proposals/{}/integrate", rid(&p.json["id"])), json!({})).await;

    for ws in ["w1", "w2"] {
        alice.post("/v1/repos/acme/web/workspaces", json!({"name": ws, "stream": "main"})).await;
    }
    let r1 = alice
        .post(
            "/v1/repos/acme/web/workspaces/w1/changes",
            json!({"changes": [{"path": "f.txt", "op": "put", "content": b64("one\nTWO-from-w1\nthree\n")}]}),
        )
        .await;
    let r2 = alice
        .post(
            "/v1/repos/acme/web/workspaces/w2/changes",
            json!({"changes": [{"path": "f.txt", "op": "put", "content": b64("one\nTWO-from-w2\nthree\n")}]}),
        )
        .await;
    let p1 = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": rid(&r1.json["head"]), "title": "w1"}))
        .await;
    let p2 = alice
        .post("/v1/repos/acme/web/proposals", json!({"stream": "main", "head": rid(&r2.json["head"]), "title": "w2"}))
        .await;
    let res = carol.post(&format!("/v1/repos/acme/web/proposals/{}/integrate", rid(&p1.json["id"])), json!({})).await;
    assert_eq!(res.json["outcome"]["status"], "integrated");

    // w2 now conflicts; the API says exactly where, and w2 is untouched.
    let prep = carol.post(&format!("/v1/repos/acme/web/proposals/{}/prepare", rid(&p2.json["id"])), json!({})).await;
    assert_eq!(prep.json["conflicts"][0]["path"], "f.txt", "{}", prep.json);
    let sync = alice.post("/v1/repos/acme/web/workspaces/w2/sync", json!({})).await;
    assert_eq!(sync.json["status"], "conflicted", "{}", sync.json);
    assert_eq!(alice.get("/v1/repos/acme/web/workspaces/w2").await.json["head"], rid(&r2.json["head"]));
    // Resolving every conflicting path yields a proper merge changeset ...
    let sync = alice
        .post(
            "/v1/repos/acme/web/workspaces/w2/sync",
            json!({"resolutions": [{"path": "f.txt", "op": "put", "content": b64("one\nTWO-resolved\nthree\n")}]}),
        )
        .await;
    assert_eq!(sync.json["status"], "merged", "{}", sync.json);
    // ... which the stream accepts as a fast-forward.
    let up = alice
        .call(
            "PATCH",
            &format!("/v1/repos/acme/web/proposals/{}", rid(&p2.json["id"])),
            Some(json!({"new_head": rid(&sync.json["head"])})),
            &[],
        )
        .await;
    assert_eq!(up.status, 200, "{}", up.json);
    let res = carol.post(&format!("/v1/repos/acme/web/proposals/{}/integrate", rid(&p2.json["id"])), json!({})).await;
    assert_eq!(res.json["outcome"]["status"], "integrated", "{}", res.json);
    assert_eq!(res.json["outcome"]["strategy"], "fast_forward");
    let f = alice.get("/v1/repos/acme/web/files/f.txt?rev=main").await;
    assert_eq!(f.body, b"one\nTWO-resolved\nthree\n");
    srv.stop().await;
}
