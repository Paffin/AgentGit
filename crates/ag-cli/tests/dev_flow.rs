//! The first five minutes with `ag-server dev`, as docs/getting-started.md tells them: log in with the token file the
//! server wrote, clone the demo repository, change a file, push, land it on the server; then see what the demo
//! policy says about an agent. The server side is `ag_server::dev::bootstrap` on a real in-process server, the
//! client side is the real `ag` binary.
#![allow(missing_docs)]

mod common;

use common::{Server, run_in};

#[test]
fn from_nothing_to_a_landed_change_with_no_hand_made_token() {
    let server = Server::start();
    let data = tempfile::tempdir().unwrap();
    let done = ag_server::dev::bootstrap(&server.app, data.path()).unwrap();
    assert!(done.repo_created && done.seeded && done.policy_applied);

    let work = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let ag = |dir: &std::path::Path, args: &[&str]| run_in(dir, cfg.path(), "user:me", args, &[]);

    // ag login URL --token-file FILE
    let token_file = done.token_file.to_str().unwrap();
    ag(work.path(), &["login", &server.url, "--token-file", token_file]).ok();
    // ag clone URL/demo/welcome
    ag(work.path(), &["clone", &format!("{}/demo/welcome", server.url)]).ok();
    let clone = work.path().join("welcome");
    assert!(clone.join("src/greeting.py").is_file() && clone.join("README.md").is_file());

    // Edit, snapshot, push, land.
    std::fs::write(clone.join("src/greeting.py"), "def greet(name: str) -> str:\n    return f\"Hi, {name}!\"\n")
        .unwrap();
    ag(&clone, &["snapshot", "-m", "A friendlier greeting"]).ok();
    let pushed = ag(&clone, &["--json", "push", "-t", "A friendlier greeting"]).ok().json();
    let id = pushed["proposal"]["id"].as_str().unwrap().to_string();
    // The demo policy lets a person land their own change.
    ag(&clone, &["remote", "integrate", &id]).ok();
    ag(&clone, &["pull"]).ok();
    assert!(std::fs::read_to_string(clone.join("src/greeting.py")).unwrap().contains("Hi, {name}"));

    // The policy on the server, in plain language, from the clone.
    let said = ag(&clone, &["remote", "policy", "main", "--explain"]).ok().stdout();
    assert!(said.contains("An agent may land such a change itself") && said.contains("`docs/**`"), "{said}");

    // A change to a protected file is held for a second pair of eyes, and the refusal says what to do.
    std::fs::write(clone.join("SECURITY.md"), "# Security\n\nReport problems to security@example.org.\n").unwrap();
    ag(&clone, &["snapshot", "-m", "Contact address"]).ok();
    let pushed = ag(&clone, &["--json", "push", "-t", "Contact address"]).ok().json();
    let id = pushed["proposal"]["id"].as_str().unwrap().to_string();
    let blocked = ag(&clone, &["remote", "integrate", &id]).code_is(3);
    let out = blocked.stdout();
    assert!(out.contains("approvals (SECURITY.md)") || out.contains("approvals (default)"), "{out}");
    assert!(out.contains(&format!("ag remote review {id} --approve")), "{out}");

    // Starting again replaces the credential; the saved login then says so rather than failing obscurely.
    let again = ag_server::dev::bootstrap(&server.app, data.path()).unwrap();
    assert_ne!(again.token, done.token);
    assert!(!again.repo_created && !again.seeded && !again.policy_applied, "{again:?}");
    assert_eq!(again.revoked, 1);
    let stale = ag(&clone, &["fetch"]).code_is(1);
    assert!(!stale.stderr().is_empty());
    ag(work.path(), &["login", &server.url, "--token-file", token_file]).ok();
    ag(&clone, &["fetch"]).ok();
}
