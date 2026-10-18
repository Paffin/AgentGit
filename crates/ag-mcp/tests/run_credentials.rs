//! Each run `ag-mcp` starts gets a credential of its own, bound to the run (ADR-0010): its calls use it, it never
//! reaches the model, and it dies with the run.
#![allow(missing_docs)]

mod common;

use common::*;
use serde_json::{Value, json};

const REPO: &str = "acme/web";

fn credentials(srv: &TestServer, subject: &str) -> Vec<Value> {
    let r = srv.admin().get(&format!("/v1/credentials?subject={subject}&include_revoked=true"));
    assert_eq!(r.0, 200, "{}", r.1);
    r.1["credentials"].as_array().unwrap().clone()
}

fn bound_to<'a>(all: &'a [Value], run: &str) -> Vec<&'a Value> {
    all.iter().filter(|c| c["scope"]["run_id"] == run).collect()
}

#[test]
fn every_run_gets_a_credential_of_its_own_that_dies_with_it() {
    let srv = TestServer::start();
    srv.repo(REPO);
    let agent = srv.credential("agent:coder", REPO, "writer", 3600, &[]);
    srv.register_agent(&agent, REPO, "coder");
    let mut m = Mcp::start(&srv, &agent, &["--repo", REPO]);

    // Two runs of one agent in one session: each is bound to its own credential.
    let a = m.ok("run_start", json!({}));
    let b = m.ok("run_start", json!({}));
    let run_a = a.structured["run"]["run_id"].as_str().unwrap().to_string();
    let run_b = b.structured["run"]["run_id"].as_str().unwrap().to_string();
    assert!(!a.text.contains("carries on with the agent's own credential"), "{}", a.text);
    let all = credentials(&srv, "agent:coder");
    let (for_a, for_b) = (bound_to(&all, &run_a), bound_to(&all, &run_b));
    assert_eq!((for_a.len(), for_b.len()), (1, 1), "{all:?}");
    assert_eq!(for_a[0]["scope"]["repos"], json!([REPO]));
    assert_eq!(for_a[0]["created_by"], "agent:coder");
    assert!(for_a[0]["revoked_at_ms"].as_i64().unwrap_or(0) == 0);

    // The run's calls use it: the server sees the bound credential used, not the agent's own.
    let lease = m.ok("lease_acquire", json!({"scope": "src/**", "ttl_seconds": 600, "run_id": run_a}));
    assert_eq!(lease.structured["lease"]["holder"], format!("agent:coder#{run_a}"));
    let all = credentials(&srv, "agent:coder");
    assert!(bound_to(&all, &run_a)[0]["last_used_ms"].as_i64().unwrap() > 0, "run a's credential was never used");
    assert_eq!(bound_to(&all, &run_b)[0]["last_used_ms"].as_i64().unwrap_or(0), 0, "run b made no call");

    // Run b's credential cannot touch run a: that is the server's rule, not ag-mcp's.
    let token_b = {
        // The test mints its own bound credential for b (ag-mcp's is in its memory, by design).
        let r = srv
            .http(&agent)
            .post("/v1/credentials", json!({"ttl_secs": 300, "scope": {"repos": [REPO], "run_id": run_b}}));
        assert_eq!(r.0, 200, "{}", r.1);
        r.1["token"].as_str().unwrap().to_string()
    };
    let as_b = srv.http(&token_b);
    let r = as_b.post(&format!("/v1/repos/{REPO}/runs/{run_a}/heartbeat"), json!({}));
    assert_eq!(r.0, 403, "{}", r.1);
    assert_eq!(as_b.post(&format!("/v1/repos/{REPO}/runs/{run_b}/heartbeat"), json!({})).0, 200);

    // Ending a run ends its credentials; the other run goes on.
    m.ok("run_finish", json!({"run_id": run_a, "status": "completed"}));
    let all = credentials(&srv, "agent:coder");
    assert!(bound_to(&all, &run_a)[0]["revoked_at_ms"].as_i64().unwrap() > 0, "{all:?}");
    assert_eq!(bound_to(&all, &run_b)[0]["revoked_at_ms"].as_i64().unwrap_or(0), 0);
    m.ok("run_heartbeat", json!({"run_id": run_b}));

    // The credentials stay in the process: nothing the model can read carries them.
    let out = m.all_stdout();
    assert!(!out.contains("agt_"), "no credential may appear in what the model reads");
    m.assert_never_saw(&token_b);
}

#[test]
fn the_operator_can_switch_it_off_or_insist_on_it() {
    let srv = TestServer::start();
    srv.repo(REPO);
    let agent = srv.credential("agent:coder", REPO, "writer", 3600, &[]);
    srv.register_agent(&agent, REPO, "coder");

    // Off: the agent's own credential does everything, as before this change.
    let mut off = Mcp::start(&srv, &agent, &["--repo", REPO, "--run-credentials", "off"]);
    let run = off.ok("run_start", json!({})).structured["run"]["run_id"].as_str().unwrap().to_string();
    assert!(bound_to(&credentials(&srv, "agent:coder"), &run).is_empty());
    off.ok("lease_acquire", json!({"scope": "docs/**", "ttl_seconds": 60, "run_id": run}));

    // Required, with a credential that cannot bind (a person's: only an agent binds its own run): refused, and
    // the run that was started for it is ended as failed rather than left behind unbound.
    let alice = srv.credential("user:alice", REPO, "writer", 3600, &[]);
    let mut req = Mcp::start(&srv, &alice, &["--repo", REPO, "--run-credentials", "required"]);
    let refused = req.call("run_start", json!({"agent": "coder"}));
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("run_credential_required") || refused.text.contains("required"), "{}", refused.text);
    let runs = srv.admin().get(&format!("/v1/repos/{REPO}/runs?agent=coder"));
    let statuses: Vec<&str> =
        runs.1["runs"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(statuses.iter().filter(|s| **s == "failed").count(), 1, "{statuses:?}");

    // Auto, the same credential: the run starts, and the result says that it is not bound.
    let mut auto = Mcp::start(&srv, &alice, &["--repo", REPO]);
    let r = auto.ok("run_start", json!({"agent": "coder"}));
    assert!(r.text.contains("only an agent can bind its own run"), "{}", r.text);
    // A value that is not one of the three is refused at start-up.
    let (code, _, err) =
        run_once(&["--server", &srv.url, "--run-credentials", "sometimes"], &[("AG_MCP_TOKEN", agent.as_str())]);
    assert_ne!(code, 0);
    assert!(err.contains("auto, required or off"), "{err}");
}
