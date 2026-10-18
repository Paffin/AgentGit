//! The `delegator` role (ADR-0010): an orchestrator that mints credentials for agents without being an
//! administrator - and cannot mint anything else, or anything wider than it holds.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};

const REPO: &str = "acme/web";

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}")).to_string()
}

fn ops_of(credential: &Value) -> BTreeSet<String> {
    credential["scope"]["operations"].as_array().unwrap().iter().map(s).collect()
}

struct World {
    srv: TestServer,
    admin: Http,
    /// `service:orchestrator`, delegator on `acme/*`: no administrator.
    orch: Http,
}

async fn world_with(tweak: impl FnOnce(&mut ag_server::config::ServerConfig)) -> World {
    let srv = TestServer::start_with(|cfg| {
        cfg.agents.min_stale_after_secs = 1;
        tweak(cfg);
    })
    .await;
    let admin = srv.admin();
    for repo in [REPO, "acme/api", "other/web"] {
        assert_eq!(admin.post("/v1/repos", json!({"name": repo})).await.status, 200);
    }
    for (subject, repo, role) in [
        ("service:orchestrator", "acme/*", "delegator"),
        ("agent:bot", "acme/*", "writer"),
        ("agent:other", "acme/*", "writer"),
        ("agent:boss", "acme/*", "admin"),
        ("user:alice", "acme/*", "writer"),
        ("service:ci-1", "acme/*", "runner"),
    ] {
        let r = admin.post("/v1/roles", json!({"subject": subject, "repo": repo, "role": role})).await;
        assert_eq!(r.status, 200, "{}", r.json);
    }
    let orch = token_for(&srv, &admin, "service:orchestrator").await;
    World { srv, admin, orch }
}

async fn world() -> World {
    world_with(|_| {}).await
}

async fn token_for(srv: &TestServer, admin: &Http, subject: &str) -> Http {
    let r = admin.post("/v1/credentials", json!({"subject": subject, "ttl_secs": 900})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    srv.client(r.json["token"].as_str().unwrap())
}

async fn mint(from: &Http, subject: &str, scope: Value) -> Reply {
    from.post("/v1/credentials", json!({"subject": subject, "ttl_secs": 600, "scope": scope})).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegator_mints_credentials_for_agents_and_only_for_agents() {
    let w = world().await;

    // An agent credential, with the scope of one repository: the delegator needs no administrator rights.
    let r = mint(&w.orch, "agent:bot", json!({"repos": [REPO]})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let ops = ops_of(&r.json["credential"]);
    // What it carries is explicit: what the delegator holds there, less everything administrative.
    for must in ["agent.run", "objects.write", "workspace.write", "proposal.create", "lease.write", "task.write"] {
        assert!(ops.contains(must), "{must} is missing from {ops:?}");
    }
    for never in [
        "credentials.mint",
        "credentials.delegate",
        "credentials.list",
        "credentials.revoke",
        "agent.operate",
        "roles.manage",
        "policy.write",
        "repo.create",
        "repo.configure",
        "integrate.run",
        "proposal.review",
        "gc.run",
    ] {
        assert!(!ops.contains(never), "{never} must not be delegated: {ops:?}");
    }
    let bot = w.srv.client(r.json["token"].as_str().unwrap());
    let ws = bot.post(&format!("/v1/repos/{REPO}/workspaces"), json!({"name": "ws", "stream": "main"})).await;
    assert_eq!(ws.status, 200, "the delegated credential works: {}", ws.json);
    assert_eq!(bot.get("/v1/repos/acme/api").await.status, 403, "and only where it was scoped");

    // Nobody but an agent: not a person, not a service, not an administrator, not a system identity.
    for subject in ["user:alice", "user:bootstrap-admin", "service:ci-1", "service:other", "system:jobs"] {
        let r = mint(&w.orch, subject, json!({"repos": [REPO]})).await;
        assert!(matches!(r.status, 403 | 400), "{subject}: {} {}", r.status, r.json);
        assert!(r.json.get("token").is_none(), "{subject}: no token may come back: {}", r.json);
    }
    let r = mint(&w.orch, "agent:bot#run_1", json!({"repos": [REPO]})).await;
    assert_eq!(r.status, 400, "{}", r.json);

    // It may not hand down a capability it does not hold, or one that is never delegated.
    for op in [
        "integrate.run",
        "queue.run",
        "proposal.review",
        "credentials.mint",
        "credentials.delegate",
        "credentials.revoke",
        "agent.operate",
        "roles.manage",
        "policy.write",
        "repo.create",
        "repo.configure",
        "gc.run",
        "audit.read",
    ] {
        let r = mint(&w.orch, "agent:bot", json!({"repos": [REPO], "operations": [op]})).await;
        assert_eq!(r.status, 403, "{op}: {}", r.json);
        assert!(r.json.get("token").is_none());
    }
    // ... but a share of what it holds, yes, exactly as asked.
    let r = mint(&w.orch, "agent:bot", json!({"repos": [REPO], "operations": ["workspace.write", "agent.run"]})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(ops_of(&r.json["credential"]), BTreeSet::from(["workspace.write".to_string(), "agent.run".to_string()]));

    // Only where it is a delegator: acme/*, not everywhere, not elsewhere.
    for repos in [json!(["acme/*"]), json!(["acme/api"]), json!([REPO, "acme/api"])] {
        assert_eq!(mint(&w.orch, "agent:bot", json!({"repos": repos})).await.status, 200, "{repos}");
    }
    for repos in [json!([]), json!(["*"]), json!(["other/web"]), json!([REPO, "other/web"]), json!(["a*"])] {
        let r = mint(&w.orch, "agent:bot", json!({ "repos": repos })).await;
        assert_eq!(r.status, 403, "{repos}: {}", r.json);
    }

    // Narrowing its own credential narrows what it can pass on.
    let narrow = w
        .orch
        .post(
            "/v1/credentials",
            json!({"ttl_secs": 300, "scope": {"repos": [REPO], "operations": ["credentials.delegate", "agent.run"]}}),
        )
        .await;
    assert_eq!(narrow.status, 200, "{}", narrow.json);
    let narrow = w.srv.client(narrow.json["token"].as_str().unwrap());
    let r = mint(&narrow, "agent:bot", json!({"repos": [REPO], "operations": ["workspace.write"]})).await;
    assert_eq!(r.status, 403, "{}", r.json);
    let r = mint(&narrow, "agent:bot", json!({"repos": [REPO]})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(ops_of(&r.json["credential"]), BTreeSet::from(["agent.run".to_string()]));
    let r = mint(&narrow, "agent:bot", json!({"repos": ["acme/api"]})).await;
    assert_eq!(r.status, 403, "outside its own credential's repositories: {}", r.json);

    // Plain writers (and the people and agents who are not delegators) still cannot mint for others.
    let alice = token_for(&w.srv, &w.admin, "user:alice").await;
    assert_eq!(mint(&alice, "agent:bot", json!({"repos": [REPO]})).await.status, 403);
    let other = token_for(&w.srv, &w.admin, "agent:other").await;
    assert_eq!(mint(&other, "agent:bot", json!({"repos": [REPO]})).await.status, 403);
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegator_cannot_escalate_through_an_agent_that_is_more_powerful_than_it() {
    let w = world().await;
    // agent:boss is an administrator of acme/* by its own role bindings.
    let boss_direct = token_for(&w.srv, &w.admin, "agent:boss").await;
    assert_eq!(
        boss_direct.post("/v1/roles", json!({"subject": "user:x", "repo": REPO, "role": "reader"})).await.status,
        200
    );

    // A credential the delegator mints for it carries only what the delegator could do itself.
    let r = mint(&w.orch, "agent:boss", json!({"repos": [REPO]})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let boss = w.srv.client(r.json["token"].as_str().unwrap());
    let attempts: Vec<(&str, Reply)> = vec![
        ("grant a role", boss.post("/v1/roles", json!({"subject": "agent:boss", "repo": "*", "role": "admin"})).await),
        ("mint for a person", boss.post("/v1/credentials", json!({"subject": "user:alice", "ttl_secs": 60})).await),
        (
            "mint for an agent",
            boss.post("/v1/credentials", json!({"subject": "agent:bot", "ttl_secs": 60, "scope": {"repos": [REPO]}}))
                .await,
        ),
        (
            "set the policy",
            boss.call(
                "PUT",
                &format!("/v1/repos/{REPO}/policy"),
                Some(json!({"stream": "main", "document": "{}"})),
                &[],
            )
            .await,
        ),
        ("read the audit log", boss.get(&format!("/v1/repos/{REPO}/audit")).await),
        ("run gc", boss.post(&format!("/v1/repos/{REPO}/gc"), json!({"dry_run": true})).await),
        ("create a repository", boss.post("/v1/repos", json!({"name": "acme/new"})).await),
        ("reap runs", boss.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({})).await),
    ];
    for (what, r) in attempts {
        assert_eq!(r.status, 403, "an agent credential from a delegator must not {what}: {}", r.json);
    }
    // What the delegator itself may do, the credential may: that is the ceiling.
    assert_eq!(
        boss.post(&format!("/v1/repos/{REPO}/workspaces"), json!({"name": "ws", "stream": "main"})).await.status,
        200
    );

    // The delegator cannot do the administrators' work itself, either.
    let o = &w.orch;
    assert_eq!(
        o.post("/v1/roles", json!({"subject": "service:orchestrator", "repo": "*", "role": "admin"})).await.status,
        403
    );
    assert_eq!(o.post("/v1/repos", json!({"name": "acme/new"})).await.status, 403);
    assert_eq!(o.get(&format!("/v1/repos/{REPO}/audit")).await.status, 403);
    assert_eq!(
        o.call("PUT", &format!("/v1/repos/{REPO}/policy"), Some(json!({"stream": "main", "document": "{}"})), &[])
            .await
            .status,
        403
    );
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegator_lists_and_revokes_the_credentials_of_agents_and_no_others() {
    let w = world().await;
    let minted = mint(&w.orch, "agent:bot", json!({"repos": [REPO]})).await;
    let (id, token) = (s(&minted.json["credential"]["id"]), s(&minted.json["token"]));
    let alice = token_for(&w.srv, &w.admin, "user:alice").await;
    let alice_id =
        s(&w.admin.post("/v1/credentials", json!({"subject": "user:alice", "ttl_secs": 60})).await.json["credential"]
            ["id"]);

    let all = w.orch.get("/v1/credentials?subject=*").await;
    let ids: Vec<String> = all.json["credentials"].as_array().unwrap().iter().map(|c| s(&c["id"])).collect();
    assert!(ids.contains(&id), "{ids:?}");
    assert!(!ids.contains(&alice_id), "a person's credentials are not the delegator's to see: {ids:?}");
    assert!(w.orch.get("/v1/credentials?subject=user:alice").await.json["credentials"].as_array().unwrap().is_empty());

    assert_eq!(w.orch.call("DELETE", &format!("/v1/credentials/{alice_id}"), None, &[]).await.status, 404);
    assert_eq!(alice.get("/v1/whoami").await.status, 200, "alice's credential is untouched");
    assert_eq!(w.srv.client(&token).get("/v1/whoami").await.status, 200);
    assert_eq!(w.orch.call("DELETE", &format!("/v1/credentials/{id}"), None, &[]).await.status, 200);
    assert_eq!(w.srv.client(&token).get("/v1/whoami").await.status, 401);
    w.srv.stop().await;
}

async fn start_run(w: &World, agent: &str, task: Option<&str>) -> String {
    let body = match task {
        Some(t) => json!({"agent": agent, "task_id": t}),
        None => json!({"agent": agent}),
    };
    let r = w.orch.post(&format!("/v1/repos/{REPO}/runs"), body).await;
    assert_eq!(r.status, 200, "{}", r.json);
    s(&r.json["run_id"])
}

async fn register(w: &World, agent: &str) {
    let key = ag_crypto::KeyPair::generate().unwrap();
    let r = w
        .orch
        .post(&format!("/v1/repos/{REPO}/agents"), json!({"name": agent, "public_key": key.public().to_hex()}))
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
}

async fn audit(w: &World, action: &str) -> Vec<Value> {
    let r = w.admin.get(&format!("/v1/repos/{REPO}/audit?contains={action}&limit=200")).await;
    r.json["entries"].as_array().unwrap().iter().filter(|e| e["action"] == action).cloned().collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_finishes_a_task_that_belongs_to_somebody_else_and_it_is_on_record() {
    let w = world().await;
    register(&w, "bot").await;
    let task = s(&w.admin.post(&format!("/v1/repos/{REPO}/tasks"), json!({"title": "stuck"})).await.json["id"]);
    start_run(&w, "bot", Some(&task)).await;
    let alice = token_for(&w.srv, &w.admin, "user:alice").await;

    // A writer is not the owner and has no operator rights.
    let r = alice.post(&format!("/v1/repos/{REPO}/tasks/{task}/finish"), json!({"state": "cancelled"})).await;
    assert_eq!(r.status, 412, "{}", r.json);
    // The orchestrator is an operator.
    let r = w.orch.post(&format!("/v1/repos/{REPO}/tasks/{task}/finish"), json!({"state": "cancelled"})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["state"], "cancelled");
    let entries = audit(&w, "task.finish").await;
    let e = entries.iter().find(|e| e["target"] == task.as_str()).expect("the finish is audited");
    assert_eq!(e["actor"], "service:orchestrator", "{e}");
    assert_eq!(e["details"]["operator"], true);
    assert_eq!(e["details"]["owner"], "agent:bot");
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_reaps_stale_runs_their_leases_and_their_credentials() {
    let w = world().await;
    register(&w, "bot").await;
    let (stale, alive) = (start_run(&w, "bot", None).await, start_run(&w, "bot", None).await);
    let lease = w.orch.post(&format!("/v1/repos/{REPO}/leases"), json!({"scope": "src/**", "ttl_ms": 600000})).await;
    assert_eq!(lease.status, 200, "{}", lease.json);
    // The stale run's lease, taken as the run (the agent's own credential acting as it).
    let bot = token_for(&w.srv, &w.admin, "agent:bot").await;
    let held = bot
        .call(
            "POST",
            &format!("/v1/repos/{REPO}/leases"),
            Some(json!({"scope": "docs/**", "ttl_ms": 600000})),
            &[("ag-run", stale.as_str())],
        )
        .await;
    assert_eq!(held.status, 200, "{}", held.json);
    let held_id = s(&held.json["lease"]["id"]);
    let bound = mint(&w.orch, "agent:bot", json!({"repos": [REPO], "run_id": stale})).await;
    assert_eq!(bound.status, 200, "{}", bound.json);
    let bound = w.srv.client(bound.json["token"].as_str().unwrap());

    tokio::time::sleep(Duration::from_millis(1300)).await;
    let hb = w.orch.post(&format!("/v1/repos/{REPO}/runs/{alive}/heartbeat"), json!({})).await;
    assert_eq!(hb.status, 200, "{}", hb.json);

    // The live run beats again right before each reap: a slow machine (Windows CI) must not let it go stale
    // between the heartbeat above and the reaping below.
    let beat = || async { w.orch.post(&format!("/v1/repos/{REPO}/runs/{alive}/heartbeat"), json!({})).await };

    // Only an operator reaps; nobody asks for a threshold below the configured minimum.
    let writer = token_for(&w.srv, &w.admin, "user:alice").await;
    for who in [&writer, &bot] {
        let r = who.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 1000})).await;
        assert_eq!(r.status, 403, "{}", r.json);
    }
    let r = w.orch.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 500})).await;
    assert_eq!(r.status, 400, "{}", r.json);

    // A dry run reports and changes nothing.
    assert_eq!(beat().await.status, 200);
    let plan =
        w.orch.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 1000, "dry_run": true})).await;
    assert_eq!(plan.status, 200, "{}", plan.json);
    assert_eq!(plan.json["dry_run"], true);
    let planned: Vec<String> = plan.json["runs"].as_array().unwrap().iter().map(|r| s(&r["run_id"])).collect();
    assert_eq!(planned, vec![stale.clone()], "{}", plan.json);
    assert_eq!(plan.json["released_leases"], json!([held_id]), "{}", plan.json);
    assert_eq!(plan.json["revoked_credentials"], 0);
    assert_eq!(bound.get("/v1/whoami").await.status, 200, "nothing happened yet");

    assert_eq!(beat().await.status, 200);
    let done = w.orch.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 1000})).await;
    assert_eq!(done.status, 200, "{}", done.json);
    assert_eq!(done.json["runs"][0]["run_id"], stale.as_str());
    assert_eq!(done.json["runs"][0]["status"], "abandoned");
    assert_eq!(done.json["released_leases"], json!([held_id]));
    assert_eq!(done.json["revoked_credentials"], 1);
    assert_eq!(bound.get("/v1/whoami").await.status, 401, "the credential of the abandoned run is dead");
    let runs = w.orch.get(&format!("/v1/repos/{REPO}/runs?agent=bot")).await;
    let status_of =
        |id: &str| runs.json["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == id).unwrap()["status"].clone();
    assert_eq!((status_of(&stale), status_of(&alive)), (json!("abandoned"), json!("running")));
    let leases = w.orch.get(&format!("/v1/repos/{REPO}/leases?active_only=true")).await;
    let active: Vec<String> = leases.json["leases"].as_array().unwrap().iter().map(|l| s(&l["id"])).collect();
    assert!(!active.contains(&held_id) && active.contains(&s(&lease.json["lease"]["id"])), "{active:?}");

    // On record, under the operator.
    let abandoned = audit(&w, "run.abandon").await;
    assert_eq!(abandoned.len(), 1);
    assert_eq!(
        (abandoned[0]["actor"].clone(), abandoned[0]["target"].clone()),
        (json!("service:orchestrator"), json!(stale))
    );
    let reaped = audit(&w, "lease.reap").await;
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0]["actor"], "service:orchestrator");

    // The task of a reaped run can be taken again; before, a second run on it was refused.
    assert_eq!(beat().await.status, 200);
    let again = w.orch.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 1000})).await;
    assert_eq!(again.json["runs"], json!([]), "{}", again.json);
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_is_not_started_twice_while_its_first_run_lives_and_can_be_once_it_is_reaped() {
    let w = world().await;
    register(&w, "bot").await;
    let task = s(&w.admin.post(&format!("/v1/repos/{REPO}/tasks"), json!({"title": "one at a time"})).await.json["id"]);
    let first = start_run(&w, "bot", Some(&task)).await;
    let second = w.orch.post(&format!("/v1/repos/{REPO}/runs"), json!({"agent": "bot", "task_id": task})).await;
    assert_eq!(second.status, 412, "{}", second.json);
    assert!(second.json["error"]["message"].as_str().unwrap().contains(&first), "{}", second.json);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let reaped = w.orch.post(&format!("/v1/repos/{REPO}/runs/reap"), json!({"stale_after_ms": 1000})).await;
    assert_eq!(reaped.json["runs"].as_array().unwrap().len(), 1, "{}", reaped.json);
    start_run(&w, "bot", Some(&task)).await;
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_reaps_stale_runs_by_itself_when_it_is_told_to() {
    let w = world_with(|cfg| {
        cfg.agents.min_stale_after_secs = 1;
        cfg.agents.run_stale_after_secs = 1;
        cfg.agents.reap_interval_secs = 1;
    })
    .await;
    register(&w, "bot").await;
    let run = start_run(&w, "bot", None).await;
    let bound = mint(&w.orch, "agent:bot", json!({"repos": [REPO], "run_id": run})).await;
    let bound = w.srv.client(bound.json["token"].as_str().unwrap());
    // The test server is started the way `ag-server serve` does it; the reaper is part of serving.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let runs = w.orch.get(&format!("/v1/repos/{REPO}/runs?agent=bot")).await;
        if runs.json["runs"][0]["status"] == "abandoned" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the run was never reaped: {}", runs.json);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let abandoned = audit(&w, "run.abandon").await;
    assert_eq!(abandoned[0]["actor"], "system:reaper", "{}", abandoned[0]);
    assert_eq!(bound.get("/v1/whoami").await.status, 401);
    w.srv.stop().await;
}
