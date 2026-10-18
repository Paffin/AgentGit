//! Credentials bound to a run (ADR-0010): minting, enforcement on every agent-scoped operation, and the end of the
//! credential with the run.

mod common;

use std::io::{BufRead, BufReader};
use std::time::Duration;

use ag_crypto::KeyPair;
use base64::Engine as _;
use common::*;
use serde_json::{Value, json};

const REPO: &str = "acme/web";

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}")).to_string()
}

struct World {
    srv: TestServer,
    admin: Http,
    /// A name-bound credential of `agent:bot` (the pre-ADR-0010 kind; it must keep working).
    bot: Http,
    /// The orchestrator: delegator role, so it mints the agents' credentials.
    orch: Http,
}

async fn world() -> World {
    world_with(|_| {}).await
}

async fn world_with(tweak: impl FnOnce(&mut ag_server::config::ServerConfig)) -> World {
    let srv = TestServer::start_with(|cfg| {
        cfg.agents.min_stale_after_secs = 1;
        tweak(cfg);
    })
    .await;
    let admin = srv.admin();
    assert_eq!(admin.post("/v1/repos", json!({"name": REPO})).await.status, 200);
    for (subject, role) in [("agent:bot", "writer"), ("agent:other", "writer"), ("service:orchestrator", "delegator")] {
        let r = admin.post("/v1/roles", json!({"subject": subject, "repo": "acme/*", "role": role})).await;
        assert_eq!(r.status, 200, "{}", r.json);
    }
    let bot = mint_for(&srv, &admin, "agent:bot").await;
    let orch = mint_for(&srv, &admin, "service:orchestrator").await;
    let key = KeyPair::generate().unwrap();
    let r = bot
        .post(&format!("/v1/repos/{REPO}/agents"), json!({"name": "bot", "public_key": key.public().to_hex()}))
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    World { srv, admin, bot, orch }
}

async fn mint_for(srv: &TestServer, admin: &Http, subject: &str) -> Http {
    let r = admin.post("/v1/credentials", json!({"subject": subject, "ttl_secs": 900})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    srv.client(r.json["token"].as_str().unwrap())
}

impl World {
    /// Starts a run of `agent:bot` with the agent's own credential.
    async fn start_run(&self) -> String {
        let r = self.bot.post(&format!("/v1/repos/{REPO}/runs"), json!({"agent": "bot"})).await;
        assert_eq!(r.status, 200, "{}", r.json);
        s(&r.json["run_id"])
    }

    /// The credential the orchestrator hands to the session of `run`: bound to it.
    async fn bound(&self, run: &str) -> (Http, String) {
        let r = self
            .orch
            .post(
                "/v1/credentials",
                json!({"subject": "agent:bot", "ttl_secs": 600, "scope": {"repos": [REPO], "run_id": run}}),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.json);
        assert_eq!(r.json["credential"]["scope"]["run_id"], run, "{}", r.json);
        (self.srv.client(r.json["token"].as_str().unwrap()), s(&r.json["credential"]["id"]))
    }

    async fn audit(&self, needle: &str) -> Vec<Value> {
        let r = self.admin.get(&format!("/v1/repos/{REPO}/audit?contains={needle}&limit=200")).await;
        assert_eq!(r.status, 200, "{}", r.json);
        r.json["entries"].as_array().unwrap().iter().filter(|e| e["action"] == needle).cloned().collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_bound_credential_acts_only_as_its_run() {
    let w = world().await;
    let (run_a, run_b) = (w.start_run().await, w.start_run().await);
    let (ca, _) = w.bound(&run_a).await;
    let (cb, _) = w.bound(&run_b).await;

    // It says what it is bound to.
    let me = ca.get("/v1/whoami").await;
    assert_eq!(me.json["identity"], "agent:bot", "{}", me.json);
    assert_eq!(me.json["scope"]["run_id"], run_a.as_str());
    assert_eq!(me.json["scope"]["repos"], json!([REPO]));

    // Run-level operations are its own run's and no other run's.
    let hb = |c: &Http, run: &str| {
        let (c, run) = (c.clone(), run.to_string());
        async move { c.post(&format!("/v1/repos/{REPO}/runs/{run}/heartbeat"), json!({})).await }
    };
    assert_eq!(hb(&ca, &run_a).await.status, 200);
    let other = hb(&ca, &run_b).await;
    assert_eq!(other.status, 403, "{}", other.json);
    assert!(other.json["error"]["message"].as_str().unwrap().contains(&run_a), "{}", other.json);
    let end = ca.post(&format!("/v1/repos/{REPO}/runs/{run_b}/complete"), json!({"status": "failed"})).await;
    assert_eq!(end.status, 403, "{}", end.json);
    assert_eq!(w.orch.get(&format!("/v1/repos/{REPO}/runs?agent=bot")).await.json["runs"].as_array().unwrap().len(), 2);

    // It always acts as its run - without `ag-run` - and refuses to act as another.
    let ws = ca.post(&format!("/v1/repos/{REPO}/workspaces"), json!({"name": "ws-a", "stream": "main"})).await;
    assert_eq!(ws.status, 200, "{}", ws.json);
    assert_eq!(ws.json["owner"], format!("agent:bot#{run_a}"), "{}", ws.json);
    let as_b = [("ag-run", run_b.as_str())];
    let refused = ca
        .call("POST", &format!("/v1/repos/{REPO}/workspaces"), Some(json!({"name": "ws-x", "stream": "main"})), &as_b)
        .await;
    assert_eq!(refused.status, 403, "{}", refused.json);
    // Naming its own run is fine (SDKs always send the header).
    let as_a = [("ag-run", run_a.as_str())];
    let ok = ca
        .call("POST", &format!("/v1/repos/{REPO}/workspaces"), Some(json!({"name": "ws-y", "stream": "main"})), &as_a)
        .await;
    assert_eq!(ok.status, 200, "{}", ok.json);

    // Another run of the same agent cannot reach what this run made: the workspace ...
    let edit = json!({"message": "m", "changes": [{"path": "f.txt", "op": "put", "content": b64("x\n")}]});
    let theirs = cb.post(&format!("/v1/repos/{REPO}/workspaces/ws-a/changes"), edit.clone()).await;
    assert_eq!(theirs.status, 403, "{}", theirs.json);
    let mine = ca.post(&format!("/v1/repos/{REPO}/workspaces/ws-a/changes"), edit).await;
    assert_eq!(mine.status, 200, "{}", mine.json);
    // ... the proposal made from it ...
    let prop = ca
        .post(
            &format!("/v1/repos/{REPO}/proposals"),
            json!({"stream": "main", "head": s(&mine.json["head"]), "title": "from run a"}),
        )
        .await;
    assert_eq!(prop.status, 200, "{}", prop.json);
    assert_eq!(prop.json["author"], format!("agent:bot#{run_a}"));
    let pid = s(&prop.json["id"]);
    let upd = cb
        .call(
            "PATCH",
            &format!("/v1/repos/{REPO}/proposals/{pid}"),
            Some(json!({"new_head": s(&mine.json["head"])})),
            &[],
        )
        .await;
    assert_eq!(upd.status, 403, "{}", upd.json);
    let close = cb.post(&format!("/v1/repos/{REPO}/proposals/{pid}/close"), json!({})).await;
    assert_eq!(close.status, 403, "{}", close.json);
    // ... and its leases.
    let lease = ca.post(&format!("/v1/repos/{REPO}/leases"), json!({"scope": "src/**", "ttl_ms": 60000})).await;
    assert_eq!(lease.status, 200, "{}", lease.json);
    assert_eq!(lease.json["lease"]["holder"], format!("agent:bot#{run_a}"));
    let lid = s(&lease.json["lease"]["id"]);
    assert_eq!(cb.call("DELETE", &format!("/v1/repos/{REPO}/leases/{lid}"), None, &[]).await.status, 403);
    assert_eq!(cb.post(&format!("/v1/repos/{REPO}/leases/{lid}/extend"), json!({"ttl_ms": 1000})).await.status, 403);
    assert_eq!(ca.post(&format!("/v1/repos/{REPO}/leases/{lid}/extend"), json!({"ttl_ms": 60000})).await.status, 200);

    // A run does not start runs.
    let start = ca.post(&format!("/v1/repos/{REPO}/runs"), json!({"agent": "bot"})).await;
    assert_eq!(start.status, 403, "{}", start.json);

    // The name-bound credential keeps working as before: it is the agent, for every run of the agent.
    assert_eq!(hb(&w.bot, &run_a).await.status, 200);
    assert_eq!(hb(&w.bot, &run_b).await.status, 200);
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_finishes_only_its_own_task_and_signs_only_its_own_provenance() {
    let w = world().await;
    let admin_task = w.admin.post(&format!("/v1/repos/{REPO}/tasks"), json!({"title": "task one"})).await;
    let other_task = w.admin.post(&format!("/v1/repos/{REPO}/tasks"), json!({"title": "task two"})).await;
    let (t1, t2) = (s(&admin_task.json["id"]), s(&other_task.json["id"]));
    let run = |task: String| {
        let bot = w.bot.clone();
        async move {
            let r = bot.post(&format!("/v1/repos/{REPO}/runs"), json!({"agent": "bot", "task_id": task})).await;
            assert_eq!(r.status, 200, "{}", r.json);
            s(&r.json["run_id"])
        }
    };
    let (run_a, run_b) = (run(t1.clone()).await, run(t2.clone()).await);
    let (ca, _) = w.bound(&run_a).await;
    let (cb, _) = w.bound(&run_b).await;

    // Run B owns task two, not task one - though both belong to agent:bot.
    let r = cb.post(&format!("/v1/repos/{REPO}/tasks/{t1}/finish"), json!({"state": "done"})).await;
    assert_eq!(r.status, 412, "{}", r.json);
    assert!(r.json["error"]["message"].as_str().unwrap().contains(&run_b), "{}", r.json);
    let r = ca.post(&format!("/v1/repos/{REPO}/tasks/{t1}/finish"), json!({"state": "done"})).await;
    assert_eq!(r.status, 200, "{}", r.json);

    // Provenance: a record for another run (or for none) is not this credential's to sign.
    let changeset = "ab".repeat(32);
    let record = |run: Option<&str>| {
        let mut r = json!({
            "changeset": changeset, "agent": format!("agent:bot#{run_a}"),
            "model": "m", "created_at_ms": 1_700_000_100_000i64
        });
        if let Some(run) = run {
            r["run_id"] = json!(run);
        }
        r
    };
    let dummy_sig = "00".repeat(64);
    for wrong in [Some(run_b.as_str()), None] {
        let r = ca
            .post(&format!("/v1/repos/{REPO}/provenance"), json!({"record": record(wrong), "signature": dummy_sig}))
            .await;
        assert_eq!(r.status, 403, "{wrong:?}: {}", r.json);
        assert!(r.json["error"]["message"].as_str().unwrap().contains(&run_a), "{}", r.json);
    }
    // Its own run passes that check (and then meets the real one: the signature is not valid).
    let r = ca
        .post(&format!("/v1/repos/{REPO}/provenance"), json!({"record": record(Some(&run_a)), "signature": dummy_sig}))
        .await;
    assert_ne!(r.status, 403, "{}", r.json);
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_bound_credential_manages_only_its_own_run_credentials() {
    let w = world().await;
    let (run_a, run_b) = (w.start_run().await, w.start_run().await);
    let (ca, id_a) = w.bound(&run_a).await;
    let (cb, id_b) = w.bound(&run_b).await;

    // It can mint nothing wider: not a name-bound credential, not one for another run, not for another agent. (The
    // operations are spelled out in every request, so that only the run binding decides.)
    let ops = json!(["agent.run", "task.read"]);
    let name_bound = ca.post("/v1/credentials", json!({"ttl_secs": 60, "scope": {"operations": ops}})).await;
    assert_eq!(name_bound.status, 403, "{}", name_bound.json);
    let no_run =
        ca.post("/v1/credentials", json!({"ttl_secs": 60, "scope": {"repos": [REPO], "operations": ops}})).await;
    assert_eq!(no_run.status, 403, "{}", no_run.json);
    let for_b = ca
        .post(
            "/v1/credentials",
            json!({"ttl_secs": 60, "scope": {"repos": [REPO], "run_id": run_b, "operations": ops}}),
        )
        .await;
    assert_eq!(for_b.status, 403, "{}", for_b.json);
    let for_other = ca
        .post(
            "/v1/credentials",
            json!({"subject": "agent:other", "ttl_secs": 60, "scope": {"repos": [REPO], "run_id": run_a, "operations": ops}}),
        )
        .await;
    assert_eq!(for_other.status, 403, "{}", for_other.json);
    // It can hand its run to a helper process: a credential bound to the same run, with a share of its operations.
    let child = ca
        .post(
            "/v1/credentials",
            json!({"ttl_secs": 60, "scope": {"repos": [REPO], "run_id": run_a, "operations": ops}}),
        )
        .await;
    assert_eq!(child.status, 200, "{}", child.json);
    let child_id = s(&child.json["credential"]["id"]);
    assert!(child.json["credential"]["expires_at_ms"].as_i64().unwrap() <= me_expiry(&ca).await);
    let helper = w.srv.client(child.json["token"].as_str().unwrap());
    assert_eq!(helper.post(&format!("/v1/repos/{REPO}/runs/{run_a}/heartbeat"), json!({})).await.status, 200);
    assert_eq!(helper.post(&format!("/v1/repos/{REPO}/runs/{run_b}/heartbeat"), json!({})).await.status, 403);

    // It lists and revokes the credentials of its run - not those of the agent's other runs, nor the agent's.
    let listed = ca.get("/v1/credentials?subject=agent:bot").await;
    let ids: Vec<String> = listed.json["credentials"].as_array().unwrap().iter().map(|c| s(&c["id"])).collect();
    assert!(ids.contains(&id_a) && ids.contains(&child_id), "{ids:?}");
    assert!(!ids.contains(&id_b), "another run's credential must not be listed: {ids:?}");
    assert_eq!(ca.call("DELETE", &format!("/v1/credentials/{id_b}"), None, &[]).await.status, 404);
    assert_eq!(cb.get("/v1/whoami").await.status, 200, "run b's credential is untouched");
    assert_eq!(ca.call("DELETE", &format!("/v1/credentials/{child_id}"), None, &[]).await.status, 200);
    w.srv.stop().await;
}

async fn me_expiry(c: &Http) -> i64 {
    c.get("/v1/whoami").await.json["expires_at_ms"].as_i64().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_credential_can_only_be_bound_to_a_running_run_of_its_own_agent() {
    let w = world().await;
    let run = w.start_run().await;
    let mint = |subject: &str, repos: Value, run: &str| {
        let (orch, subject, run) = (w.orch.clone(), subject.to_string(), run.to_string());
        async move {
            orch.post(
                "/v1/credentials",
                json!({"subject": subject, "ttl_secs": 60, "scope": {"repos": repos, "run_id": run}}),
            )
            .await
        }
    };
    assert_eq!(mint("agent:bot", json!([REPO]), &run).await.status, 200);
    // The scope must name exactly one literal repository: the one that holds the run.
    for repos in [json!([]), json!(["acme/*"]), json!(["*"]), json!([REPO, "acme/api"])] {
        let r = mint("agent:bot", repos.clone(), &run).await;
        assert_eq!(r.status, 400, "{repos}: {}", r.json);
    }
    // The run must exist, and belong to the subject.
    assert_eq!(mint("agent:bot", json!([REPO]), "run_nope").await.status, 404);
    let r = mint("agent:other", json!([REPO]), &run).await;
    assert_eq!(r.status, 400, "{}", r.json);
    assert!(r.json["error"]["message"].as_str().unwrap().contains("belongs to agent 'bot'"), "{}", r.json);
    // A run id is not an identity: the subject never carries one.
    let r = mint(&format!("agent:bot#{run}"), json!([REPO]), &run).await;
    assert_eq!(r.status, 400, "{}", r.json);
    // Only an agent's credential can be bound to a run.
    let r = mint("user:alice", json!([REPO]), &run).await;
    assert_ne!(r.status, 200, "{}", r.json);
    // The caller must be able to run agents there: someone without a role learns nothing about the run.
    let carol = user_with_role(&w.srv, "user:carol", REPO, "reader").await;
    let r = carol
        .post(
            "/v1/credentials",
            json!({"subject": "user:carol", "ttl_secs": 60, "scope": {"repos": [REPO], "run_id": run}}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.json);
    // A finished run takes no new credentials.
    let done = w.bot.post(&format!("/v1/repos/{REPO}/runs/{run}/complete"), json!({"status": "completed"})).await;
    assert_eq!(done.status, 200, "{}", done.json);
    let r = mint("agent:bot", json!([REPO]), &run).await;
    assert_eq!(r.status, 412, "{}", r.json);
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_bound_credential_dies_with_its_run() {
    let w = world().await;
    let (run_a, run_b) = (w.start_run().await, w.start_run().await);
    let (ca, id_a) = w.bound(&run_a).await;
    let (cb, id_b) = w.bound(&run_b).await;
    assert_eq!(ca.get("/v1/whoami").await.status, 200);

    // The run's own end ends its credentials: the answer is `unauthenticated` for everything, whoami included.
    let done = ca.post(&format!("/v1/repos/{REPO}/runs/{run_a}/complete"), json!({"status": "completed"})).await;
    assert_eq!(done.status, 200, "{}", done.json);
    for r in [
        ca.get("/v1/whoami").await,
        ca.post(&format!("/v1/repos/{REPO}/runs/{run_a}/heartbeat"), json!({})).await,
        ca.get(&format!("/v1/repos/{REPO}/tasks")).await,
    ] {
        assert_eq!(r.status, 401, "{}", r.json);
    }
    // ... and the listing shows it revoked (not just unusable), while the other run's credential is untouched.
    let creds = w.orch.get("/v1/credentials?subject=agent:bot&include_revoked=true").await;
    let by_id =
        |id: &str| creds.json["credentials"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned().unwrap();
    assert!(by_id(&id_a)["revoked_at_ms"].as_i64().unwrap() > 0, "{creds:?}", creds = creds.json);
    assert_eq!(by_id(&id_b)["revoked_at_ms"].as_i64().unwrap_or(0), 0);
    assert_eq!(cb.get("/v1/whoami").await.status, 200);

    // The name-bound credential of the agent is not affected by a run ending.
    assert_eq!(w.bot.get("/v1/whoami").await.status, 200);

    // An operator finishing a run on the agent's behalf ends its credentials too, and is on record as the operator.
    let r = w
        .orch
        .post(
            &format!("/v1/repos/{REPO}/runs/{run_b}/complete"),
            json!({"status": "failed", "summary": "session lost"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(cb.get("/v1/whoami").await.status, 401);
    let finish = w.audit("run.finish").await;
    let entry = finish.iter().find(|e| e["target"] == run_b.as_str()).expect("the operator's finish is audited");
    assert_eq!(entry["actor"], "service:orchestrator", "{entry}");
    assert_eq!(entry["details"]["operator"], true, "{entry}");
    let own = finish.iter().find(|e| e["target"] == run_a.as_str()).unwrap();
    assert_eq!(own["actor"], format!("agent:bot#{run_a}"), "the agent's own finish is the run's: {own}");
    assert!(own["details"].get("operator").is_none(), "{own}");
    w.srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_credential_stops_working_when_its_run_is_gone_even_if_it_was_never_revoked() {
    // The run ends behind the credential's back (a crash between the two steps, a replica that did not revoke).
    let w = world().await;
    let run = w.start_run().await;
    let (c, id) = w.bound(&run).await;
    assert_eq!(c.get("/v1/whoami").await.status, 200);
    let handle = w.srv.app.repos.handle(REPO).unwrap();
    ag_agent::registry::finish_run(handle.meta(), &run, "failed", Some("ended without the server noticing")).unwrap();
    // Not revoked ...
    let creds = w.orch.get("/v1/credentials?subject=agent:bot").await;
    assert!(creds.json["credentials"].as_array().unwrap().iter().any(|x| x["id"] == id.as_str()));
    // ... and useless anyway, for every operation, with the same answer as for any dead credential.
    for r in [c.get("/v1/whoami").await, c.get(&format!("/v1/repos/{REPO}/streams")).await] {
        assert_eq!(r.status, 401, "{}", r.json);
        assert!(r.json["error"]["message"].as_str().unwrap().contains("no longer running"), "{}", r.json);
    }
    w.srv.stop().await;
}

fn sse(base: String, token: String, path: String, tx: std::sync::mpsc::Sender<(String, String)>) {
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        let Ok(mut resp) = agent.get(format!("{base}{path}")).header("authorization", format!("Bearer {token}")).call()
        else {
            return;
        };
        let reader = BufReader::new(resp.body_mut().as_reader());
        let (mut event, mut data) = (String::new(), String::new());
        for line in reader.lines() {
            let Ok(line) = line else { return };
            if line.is_empty() {
                if !data.is_empty() && tx.send((event.clone(), data.clone())).is_err() {
                    return;
                }
                event.clear();
                data.clear();
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().into();
            } else if let Some(v) = line.strip_prefix("data:") {
                data = v.trim().into();
            }
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_stream_opened_with_a_run_bound_credential_ends_with_the_run() {
    let w = world().await;
    let run = w.start_run().await;
    let (c, _) = w.bound(&run).await;
    let (tx, rx) = std::sync::mpsc::channel();
    sse(w.srv.base.clone(), c.token.clone(), format!("/v1/repos/{REPO}/events"), tx);
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The stream follows while the run runs ...
    w.admin.post(&format!("/v1/repos/{REPO}/streams"), json!({"name": "one"})).await;
    let (event, _) = rx.recv_timeout(Duration::from_secs(10)).expect("an event while the run is running");
    assert_eq!(event, "stream.create");
    // ... and ends, with an error, once the run is over: the next batch is withheld.
    let done = w.orch.post(&format!("/v1/repos/{REPO}/runs/{run}/complete"), json!({"status": "completed"})).await;
    assert_eq!(done.status, 200, "{}", done.json);
    w.admin.post(&format!("/v1/repos/{REPO}/streams"), json!({"name": "two"})).await;
    let (event, data) = rx.recv_timeout(Duration::from_secs(10)).expect("the stream ends with an error event");
    assert_eq!(event, "error", "no event may follow the end of the run: {data}");
    assert!(data.contains("unauthenticated"), "{data}");
    w.srv.stop().await;
}

/// The stream's own check of the run, not the revocation that normally accompanies the end of a run: the run ends
/// behind the credential's back, so the credential is still listed as active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_stream_ends_with_the_run_even_when_its_credential_was_never_revoked() {
    let w = world().await;
    let run = w.start_run().await;
    let (c, id) = w.bound(&run).await;
    let (tx, rx) = std::sync::mpsc::channel();
    sse(w.srv.base.clone(), c.token.clone(), format!("/v1/repos/{REPO}/events"), tx);
    tokio::time::sleep(Duration::from_millis(500)).await;
    w.admin.post(&format!("/v1/repos/{REPO}/streams"), json!({"name": "one"})).await;
    let (event, _) = rx.recv_timeout(Duration::from_secs(10)).expect("an event while the run is running");
    assert_eq!(event, "stream.create");
    let handle = w.srv.app.repos.handle(REPO).unwrap();
    ag_agent::registry::finish_run(handle.meta(), &run, "failed", Some("ended without the server noticing")).unwrap();
    let creds = w.orch.get("/v1/credentials?subject=agent:bot").await;
    assert!(creds.json["credentials"].as_array().unwrap().iter().any(|x| x["id"] == id.as_str()), "still active");
    w.admin.post(&format!("/v1/repos/{REPO}/streams"), json!({"name": "two"})).await;
    let (event, data) = rx.recv_timeout(Duration::from_secs(10)).expect("the stream ends with an error event");
    assert_eq!(event, "error", "no event may follow the end of the run: {data}");
    assert!(data.contains("unauthenticated"), "{data}");
    w.srv.stop().await;
}
