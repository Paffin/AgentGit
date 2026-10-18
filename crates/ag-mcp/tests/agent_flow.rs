//! A real `ag-server` and a real `ag-mcp` over stdio: an agent works a task end to end, and is refused where it must be.
#![allow(missing_docs)]

mod common;

use common::*;
use serde_json::{Value, json};

const REPO: &str = "acme/web";

struct World {
    srv: TestServer,
    agent: String,
}

fn world() -> World {
    let srv = TestServer::start();
    srv.repo(REPO);
    srv.seed(
        REPO,
        "Initial import",
        &[
            ("README.md", "# Web\n"),
            ("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"),
            ("src/auth/login.rs", "pub fn login(user: &str) -> bool {\n    !user.is_empty()\n}\n"),
            ("docs/guide.md", "Guide\n"),
        ],
    );
    let agent = srv.credential("agent:coder", REPO, "writer", 3600, &[]);
    srv.register_agent(&agent, REPO, "coder");
    World { srv, agent }
}

fn create_task(w: &World, title: &str, description: &str, scope: &[&str]) -> String {
    let planner = w.srv.credential("user:planner", REPO, "writer", 3600, &[]);
    let r = w
        .srv
        .http(&planner)
        .post(&format!("/v1/repos/{REPO}/tasks"), json!({"title": title, "description": description, "scope": scope}));
    assert_eq!(r.0, 200, "{}", r.1);
    r.1["id"].as_str().unwrap().to_string()
}

fn text_contains_all(t: &str, parts: &[&str]) {
    for p in parts {
        assert!(t.contains(p), "missing `{p}` in:\n{t}");
    }
}

#[test]
fn an_agent_works_a_task_from_claim_to_verdicts() {
    let w = world();
    let task = create_task(&w, "Reject blank user names", "login() accepts \"  \"; it should not.", &["src/auth/**"]);
    let mut m = Mcp::start(
        &w.srv,
        &w.agent,
        &["--repo", REPO, "--write-path", "src/**", "--write-path", "docs/**", "--require-lease"],
    );

    // 1. find work
    let tasks = m.ok("task_list", json!({"ready_only": true}));
    assert_eq!(tasks.structured["tasks"][0]["id"], task.as_str());
    assert_eq!(tasks.structured["tasks"][0]["scope"], json!(["src/auth/**"]));

    // 2. claim it: a run with its own identity
    let claim = m.ok("task_claim", json!({"task_id": task}));
    let run = claim.structured["run"]["run_id"].as_str().unwrap().to_string();
    assert_eq!(claim.structured["run"]["identity"], format!("agent:coder#{run}"));
    assert_eq!(claim.structured["task"]["owner"], "agent:coder");
    assert!(claim.text.contains(&format!("run_id=\"{run}\"")), "{}", claim.text);
    // another agent's claim is refused by the server, and explained
    let rival = w.srv.credential("agent:rival", REPO, "writer", 3600, &[]);
    w.srv.register_agent(&rival, REPO, "rival");
    let mut other = Mcp::start(&w.srv, &rival, &["--repo", REPO]);
    let taken = other.call("task_claim", json!({"task_id": task}));
    assert!(taken.is_error, "a task owned by someone else cannot be claimed: {}", taken.text);
    assert!(taken.text.contains("kind=server.error") && taken.text.contains("owned by agent:coder"), "{}", taken.text);

    // 3. lease the scope (exclusive: a fencing token comes back)
    let lease = m.ok(
        "lease_acquire",
        json!({"scope": "src/auth/**", "exclusive": true, "ttl_seconds": 600, "run_id": run, "task_id": task}),
    );
    let lease_id = lease.structured["lease"]["id"].as_str().unwrap().to_string();
    assert!(lease.structured["lease"]["fencing_token"].as_u64().unwrap() >= 1);
    assert_eq!(lease.structured["lease"]["holder"], format!("agent:coder#{run}"));
    assert!(lease.structured["lease"]["expires_in_seconds"].as_i64().unwrap() > 500);
    assert_eq!(m.ok("lease_list", json!({})).structured["leases"].as_array().unwrap().len(), 1);
    let too_long = m.refused("lease_acquire", json!({"scope": "src/**", "ttl_seconds": 86400}), "lease_too_long");
    assert!(too_long.text.contains("operator limits leases to 3600"), "{}", too_long.text);

    // 4. understand, then write
    let before = m.ok("read_file", json!({"path": "src/auth/login.rs"}));
    assert!(before.structured["content"].as_str().unwrap().contains("!user.is_empty()"));
    assert!(m.ok("predict_conflicts", json!({})).structured["overlaps"].as_array().unwrap().is_empty());
    let ws = m.ok("workspace_create", json!({"name": "fix-login", "run_id": run}));
    assert_eq!(ws.structured["workspace"]["owner"], format!("agent:coder#{run}"));

    // Refusals, all before anything is written, each with the reason:
    let outside_write_paths = m.refused(
        "workspace_write_files",
        json!({"workspace": "fix-login", "message": "x", "run_id": run, "changes": [{"path": "Cargo.toml", "op": "put", "content": "[package]\n"}]}),
        "path_not_allowed",
    );
    text_contains_all(
        &outside_write_paths.text,
        &["`Cargo.toml`", "outside the paths the operator allows", "src/**, docs/**", "--write-path"],
    );
    let outside_lease = m.refused(
        "workspace_write_files",
        json!({"workspace": "fix-login", "message": "x", "run_id": run, "changes": [{"path": "src/lib.rs", "op": "put", "content": "// hijack\n"}]}),
        "path_outside_lease",
    );
    text_contains_all(
        &outside_lease.text,
        &["`src/lib.rs`", "outside the lease scope(s) you hold: src/auth/**", "--require-lease", "lease_acquire"],
    );
    let traversal = m.refused(
        "workspace_write_files",
        json!({"workspace": "fix-login", "message": "x", "changes": [{"path": "src/auth/../../etc/passwd", "op": "put", "content": "x"}]}),
        "invalid_arguments",
    );
    assert!(traversal.text.contains("`..` segments"), "{}", traversal.text);
    let wrong_lease = m.refused(
        "workspace_write_files",
        json!({"workspace": "fix-login", "message": "x", "lease_id": "lease_nope", "changes": [{"path": "src/auth/login.rs", "op": "put", "content": "x"}]}),
        "lease_not_held",
    );
    assert!(wrong_lease.text.contains("not an active lease held by agent:coder"), "{}", wrong_lease.text);
    assert_eq!(
        m.ok("log", json!({"rev": "@fix-login"})).structured["changesets"].as_array().unwrap().len(),
        1,
        "no refused write left a trace"
    );

    // The write inside the lease works: one atomic snapshot.
    let new_login = "pub fn login(user: &str) -> bool {\n    !user.trim().is_empty()\n}\n";
    let snap = m.ok(
        "workspace_write_files",
        json!({"workspace": "fix-login", "message": "login: reject blank user names", "run_id": run, "lease_id": lease_id,
               "changes": [{"path": "src/auth/login.rs", "op": "put", "content": new_login}]}),
    );
    let head = snap.structured["head"].as_str().unwrap().to_string();
    assert_eq!(
        snap.structured["files"][0],
        json!({"path": "src/auth/login.rs", "op": "put", "bytes": new_login.len()})
    );
    let in_ws = m.ok("read_file", json!({"path": "src/auth/login.rs", "rev": "@fix-login"}));
    assert_eq!(in_ws.structured["content"], new_login);
    assert_eq!(in_ws.structured["commit"], head.as_str());
    let d = m.ok("diff", json!({"from": "main", "to": "@fix-login", "since_merge_base": true}));
    assert_eq!(d.structured["files_changed"], 1);
    assert!(d.structured["files"][0]["patch"].as_str().unwrap().contains("+    !user.trim().is_empty()"));

    // 5. propose: the verdicts come back with the proposal
    let p = m.ok(
        "propose",
        json!({"workspace": "fix-login", "title": "Reject blank user names", "description": "Trim before checking.", "task_id": task, "run_id": run}),
    );
    let pid = p.structured["proposal"]["id"].as_str().unwrap().to_string();
    assert_eq!(p.structured["proposal"]["author"], format!("agent:coder#{run}"));
    let v = &p.structured["status"]["verdicts"];
    assert_eq!(v["mergeable"]["state"], "ok", "{}", p.structured);
    assert_eq!(v["validated"]["state"], "na");
    assert_eq!(v["approved"]["state"], "blocked");
    assert_eq!(v["approved"]["value"], "0/1");
    assert_eq!(v["integrated"]["state"], "pending");

    // 6. the four verdicts and the blockers in words
    let st = m.ok("proposal_status", json!({"proposal_id": pid}));
    assert_eq!(st.structured["ready_to_integrate"], false);
    let titles: Vec<String> = st.structured["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect();
    assert!(titles.contains(&"Needs 1 more approval (0 of 1)".to_string()), "{titles:?}");
    assert!(
        titles.contains(&"You are not allowed to integrate this change under the policy".to_string()),
        "{titles:?}"
    );
    let severities: Vec<&str> =
        st.structured["blockers"].as_array().unwrap().iter().map(|b| b["severity"].as_str().unwrap()).collect();
    assert!(severities.contains(&"refused") && severities.contains(&"provide"));
    text_contains_all(&st.text, &["Mergeable: clean (ok)", "Approved: 0/1 (blocked)", "Integrated: not yet (pending)"]);
    assert!(
        m.ok("request_validation", json!({"proposal_id": pid})).structured["jobs"].as_array().unwrap().is_empty(),
        "no check is required, so nothing is queued"
    );
    assert_eq!(m.ok("proposal_list", json!({"state": "open"})).structured["proposals"][0]["id"], pid.as_str());

    // 7. forbidden: an agent that was not given the integrate tool cannot call it, and is told why
    assert!(!m.tool_names().contains(&"integrate".to_string()));
    let integrate = m.refused("integrate", json!({"proposal_id": pid}), "tool_disabled");
    text_contains_all(
        &integrate.text,
        &["switched off by the operator", "group `integrate`", "nothing was sent to AgentGit", "Hand the proposal id"],
    );
    let review = m.refused("review", json!({"proposal_id": pid, "head": head, "decision": "approve"}), "tool_disabled");
    assert!(review.text.contains("an author must not review its own work"), "{}", review.text);

    // 8. a human reviews and integrates; the agent sees the verdicts flip
    let rita = w.srv.credential("user:rita", REPO, "reviewer", 3600, &[]);
    let r = w
        .srv
        .http(&rita)
        .post(&format!("/v1/repos/{REPO}/proposals/{pid}/reviews"), json!({"decision": "approve", "body": "lgtm"}));
    assert_eq!(r.0, 200, "{}", r.1);
    let st = m.ok("proposal_status", json!({"proposal_id": pid}));
    assert_eq!(st.structured["verdicts"]["approved"]["state"], "ok");
    assert_eq!(st.structured["reviews"]["approvals"][0]["reviewer"], "user:rita");
    assert_eq!(st.structured["verdicts"]["mergeable"]["state"], "ok", "approval does not touch mergeability");
    let ivan = w.srv.credential("user:ivan", REPO, "integrator", 3600, &[]);
    let r = w.srv.http(&ivan).post(
        &format!("/v1/repos/{REPO}/proposals/{pid}/integrate"),
        json!({"fencing_tokens": [lease.structured["lease"]["fencing_token"]]}),
    );
    assert_eq!(r.1["outcome"]["status"], "integrated", "{}", r.1);
    let st = m.ok("proposal_status", json!({"proposal_id": pid}));
    assert_eq!(st.structured["verdicts"]["integrated"]["state"], "ok");
    assert_eq!(st.structured["proposal"]["state"], "integrated");
    assert_eq!(
        m.ok("read_file", json!({"path": "src/auth/login.rs"})).structured["content"],
        new_login,
        "main now has the change"
    );

    // 9. close the books: the task, the run (which releases the lease)
    let done = m.ok("task_complete", json!({"task_id": task, "run_id": run}));
    assert_eq!(done.structured["task"]["state"], "done");
    m.ok("run_heartbeat", json!({"run_id": run}));
    let fin = m.ok("run_finish", json!({"run_id": run, "summary": "fixed"}));
    assert_eq!(fin.structured["run"]["status"], "completed");
    assert!(
        m.ok("lease_list", json!({})).structured["leases"].as_array().unwrap().is_empty(),
        "finishing the run released the lease"
    );
    let after = m.refused("lease_acquire", json!({"scope": "docs/**", "run_id": run}), "failed_precondition");
    assert!(after.text.contains("finished run"), "{}", after.text);
    assert!(m.ok("workspace_delete", json!({"workspace": "fix-login"})).structured["deleted"] == true);
}

#[test]
fn a_conflicting_workspace_is_reported_and_resolved_through_tools() {
    let w = world();
    let mut m = Mcp::start(&w.srv, &w.agent, &["--repo", REPO]);
    m.ok("workspace_create", json!({"name": "mine"}));
    m.ok(
        "workspace_write_files",
        json!({"workspace": "mine", "message": "my take", "changes": [{"path": "docs/guide.md", "op": "put", "content": "Guide\nMy paragraph.\n"}]}),
    );
    // Meanwhile the stream moves under it.
    w.srv.seed(REPO, "someone else's take", &[("docs/guide.md", "Guide\nTheir paragraph.\n")]);
    let sync = m.ok("workspace_sync", json!({"workspace": "mine"}));
    assert_eq!(sync.structured["status"], "conflicted", "{}", sync.structured);
    assert_eq!(sync.structured["conflicts"][0]["path"], "docs/guide.md");
    assert!(sync.text.contains("NOT changed") && sync.text.contains("resolutions"), "{}", sync.text);
    let resolved = m.ok(
        "workspace_sync",
        json!({"workspace": "mine", "resolutions": [{"path": "docs/guide.md", "content": "Guide\nTheir paragraph.\nMy paragraph.\n"}]}),
    );
    assert_eq!(resolved.structured["status"], "merged", "{}", resolved.structured);
    let p = m.ok("propose", json!({"workspace": "mine", "title": "Both paragraphs"}));
    assert_eq!(p.structured["status"]["verdicts"]["mergeable"]["state"], "ok", "{}", p.structured);
    // Edit again and move the proposal; the old head's approvals (none here) would stop counting.
    m.ok(
        "workspace_write_files",
        json!({"workspace": "mine", "message": "typo", "changes": [{"path": "docs/guide.md", "op": "put", "content": "Guide\nTheir paragraph.\nMy paragraph!\n"}]}),
    );
    let moved = m.ok("proposal_update", json!({"proposal_id": p.structured["proposal"]["id"], "workspace": "mine"}));
    assert_ne!(moved.structured["proposal"]["head"], p.structured["proposal"]["head"]);
    let closed = m.ok("proposal_close", json!({"proposal_id": p.structured["proposal"]["id"]}));
    assert_eq!(closed.structured["proposal"]["state"], "closed");
    let st = m.ok("proposal_status", json!({"proposal_id": p.structured["proposal"]["id"]}));
    assert_eq!(st.structured["verdicts"]["integrated"]["value"], "closed");
}

#[test]
fn leases_conflict_between_agents_and_the_refusal_is_explained() {
    let w = world();
    let rival = w.srv.credential("agent:rival", REPO, "writer", 3600, &[]);
    let mut a = Mcp::start(&w.srv, &w.agent, &["--repo", REPO]);
    let mut b = Mcp::start(&w.srv, &rival, &["--repo", REPO]);
    let first = a.ok("lease_acquire", json!({"scope": "src/**", "exclusive": true}));
    let clash = b.refused("lease_acquire", json!({"scope": "src/auth/**", "exclusive": true}), "lease_conflict");
    text_contains_all(&clash.text, &["Another agent holds an exclusive lease", "lease_list"]);
    // A soft lease always succeeds and reports who else is nearby.
    let soft = b.ok("lease_acquire", json!({"scope": "src/auth/**"}));
    assert_eq!(soft.structured["overlapping"][0]["id"], first.structured["lease"]["id"]);
    assert!(soft.text.contains("overlapping leases belong to others"), "{}", soft.text);
    // Only the holder can release.
    let id = first.structured["lease"]["id"].as_str().unwrap().to_string();
    let not_yours = b.call("lease_release", json!({"lease_id": id}));
    assert!(not_yours.is_error, "{}", not_yours.text);
    assert_eq!(a.ok("lease_release", json!({"lease_id": id})).structured["released"], true);
    let renew_gone = a.call("lease_renew", json!({"lease_id": id}));
    assert!(renew_gone.is_error);
}

#[test]
fn the_integrate_tool_exists_only_behind_its_flag_and_never_bypasses_policy() {
    let w = world();
    let admin = w.srv.admin();
    // Policy: agents may land small changes under src/** without approval.
    let policy = json!({"approvals": {"min": 0, "min_for_agents": 0}, "agent_autonomy": {"auto_integrate": true, "allowed_paths": ["src/**"], "max_files": 2}});
    let r = admin.call(
        "PUT",
        &format!("/v1/repos/{REPO}/policy"),
        Some(json!({"stream": "main", "document": policy.to_string()})),
    );
    assert_eq!(r.0, 200, "{}", r.1);
    w.srv.grant("agent:coder", REPO, "integrator");

    // Without the flag: not listed, refused with an explanation.
    let mut off = Mcp::start(&w.srv, &w.agent, &["--repo", REPO]);
    assert!(!off.tool_names().contains(&"integrate".to_string()));
    off.refused("integrate", json!({"proposal_id": "prop_x"}), "tool_disabled");
    // `--allow integrate` is not a way in, and `admin` does not exist.
    let (code, _, err) =
        run_once(&["--server", &w.srv.url, "--allow", "read,integrate"], &[("AG_MCP_TOKEN", &w.agent)]);
    assert_ne!(code, 0);
    assert!(err.contains("--enable-integrate"), "{err}");
    let (code, _, err) = run_once(&["--server", &w.srv.url, "--allow", "admin"], &[("AG_MCP_TOKEN", &w.agent)]);
    assert_ne!(code, 0);
    assert!(err.contains("no `admin` group"), "{err}");
    drop(off);

    // With the flag: loud at startup, and policy still decides.
    let mut m = Mcp::start(&w.srv, &w.agent, &["--repo", REPO, "--enable-integrate"]);
    let err = m.stderr_with("THE INTEGRATE TOOL IS ENABLED");
    assert!(err.contains("THE INTEGRATE TOOL IS ENABLED"), "{err}");
    assert!(m.tool_names().contains(&"integrate".to_string()));
    let integrate_tool = m.tools().into_iter().find(|t| t["name"] == "integrate").unwrap();
    assert_eq!(integrate_tool["annotations"]["destructiveHint"], true);
    assert_eq!(integrate_tool["annotations"]["readOnlyHint"], false);

    m.ok("workspace_create", json!({"name": "wide"}));
    m.ok("workspace_write_files", json!({"workspace": "wide", "message": "touches docs", "changes": [{"path": "docs/guide.md", "op": "put", "content": "Guide v2\n"}]}));
    let wide = m.ok("propose", json!({"workspace": "wide", "title": "Docs"}));
    let wide_id = wide.structured["proposal"]["id"].as_str().unwrap().to_string();
    let out = m.ok("integrate", json!({"proposal_id": wide_id}));
    assert_ne!(out.structured["status"], "integrated", "outside the autonomy envelope: {}", out.structured);
    assert!(out.text.starts_with("NOT integrated"), "{}", out.text);
    assert!(!out.structured["blockers"].as_array().unwrap().is_empty());
    assert_eq!(
        m.ok("log", json!({"limit": 1})).structured["changesets"][0]["message"],
        "Initial import",
        "the stream did not move"
    );

    m.ok("workspace_create", json!({"name": "small"}));
    m.ok("workspace_write_files", json!({"workspace": "small", "message": "in the envelope", "changes": [{"path": "src/lib.rs", "op": "put", "content": "pub fn add(a: i32, b: i32) -> i32 { b + a }\n"}]}));
    let small = m.ok("propose", json!({"workspace": "small", "title": "Commute"}));
    let small_id = small.structured["proposal"]["id"].as_str().unwrap().to_string();
    let out = m.ok("integrate", json!({"proposal_id": small_id}));
    assert_eq!(out.structured["status"], "integrated", "{}\n{}", out.text, out.structured);
    assert!(out.text.starts_with("INTEGRATED"), "{}", out.text);
    assert_eq!(m.ok("log", json!({"limit": 1})).structured["changesets"][0]["message"], "in the envelope");
    assert!(m.stderr_with("integrate tool called").contains("integrate tool called"), "every use is logged");
}

#[test]
fn review_is_available_only_when_allowed_and_binds_to_the_head_that_was_read() {
    let w = world();
    let author = w.agent.clone();
    let reviewer = w.srv.credential("agent:critic", REPO, "reviewer", 3600, &[]);
    w.srv.grant("agent:critic", REPO, "writer");
    let mut a = Mcp::start(&w.srv, &author, &["--repo", REPO]);
    a.ok("workspace_create", json!({"name": "feat"}));
    a.ok("workspace_write_files", json!({"workspace": "feat", "message": "v1", "changes": [{"path": "README.md", "op": "put", "content": "# Web v1\n"}]}));
    let p = a.ok("propose", json!({"workspace": "feat", "title": "Readme"}));
    let pid = p.structured["proposal"]["id"].as_str().unwrap().to_string();
    let head1 = p.structured["proposal"]["head"].as_str().unwrap().to_string();

    let mut r = Mcp::start(&w.srv, &reviewer, &["--repo", REPO, "--allow", "read,review"]);
    assert!(r.tool_names().contains(&"review".to_string()));
    let prompts = r.request("prompts/list", json!({}));
    assert!(prompts["result"]["prompts"].as_array().unwrap().iter().any(|p| p["name"] == "review_proposal"));
    // the author moves the proposal after the reviewer looked
    a.ok("workspace_write_files", json!({"workspace": "feat", "message": "v2", "changes": [{"path": "README.md", "op": "put", "content": "# Web v2\n"}]}));
    a.ok("proposal_update", json!({"proposal_id": pid, "workspace": "feat"}));
    let stale = r.refused("review", json!({"proposal_id": pid, "head": head1, "decision": "approve"}), "head_changed");
    text_contains_all(&stale.text, &["Nothing was recorded", "proposal_status"]);
    let st = r.ok("proposal_status", json!({"proposal_id": pid}));
    let head2 = st.structured["proposal"]["head"].as_str().unwrap().to_string();
    assert_ne!(head1, head2);
    let needs_body = r.refused(
        "review",
        json!({"proposal_id": pid, "head": head2, "decision": "request_changes"}),
        "invalid_arguments",
    );
    assert!(needs_body.text.contains("`body` is required"), "{}", needs_body.text);
    let ok = r.ok("review", json!({"proposal_id": pid, "head": head2, "decision": "approve", "body": "ok"}));
    assert_eq!(ok.structured["review"]["decision"], "approve");
    assert_eq!(ok.structured["review"]["reviewer"], "agent:critic");
    // ... and an author's own approval is not a way around the policy
    let st = r.ok("proposal_status", json!({"proposal_id": pid}));
    assert_eq!(st.structured["reviews"]["approvals"].as_array().unwrap().len(), 1);
    // the reviewer's credential cannot write: the write tools are off
    let nope: Value = r.refused("workspace_create", json!({"name": "x"}), "tool_disabled").raw;
    assert_eq!(nope["isError"], true);
}
