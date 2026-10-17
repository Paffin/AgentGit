//! The exit codes in `ag help exit-codes` and `docs/reference/cli.md` are a contract. Each row of the table
//! there is exercised here against the real binary: 0 ok, 1 error, 2 usage, 3 blocked, 4 refused.
#![allow(missing_docs)]

mod common;

use std::fs;

use ag_server::authz::Role;
use common::{Ag, Server};

/// A repository with one landed change and two open proposals that edit the same line of README.md.
fn two_competing_proposals() -> (Ag, String, String) {
    let a = Ag::new();
    a.land_first_change();
    let bob_dir = a.dir.path().join(".bob-ws");
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", bob_dir.to_str().unwrap()]).ok();
    fs::write(bob_dir.join("README.md"), "# bob's title\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob", "--workspace", "bob"]).ok();
    let pb = a.run_as("user:bob", &["--json", "propose", "@bob", "-t", "bob"]).ok().json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    a.write("README.md", "# alice's title\n");
    let pa = a.propose_as("user:alice", "alice");
    (a, pa, pb)
}

#[test]
fn zero_is_success_and_one_is_an_error_with_the_message_on_stderr() {
    let a = Ag::new();
    a.ag(&["status"]).code_is(0);
    let r = a.ag(&["integrate", "prop_does_not_exist"]).code_is(1);
    assert_eq!(r.stdout(), "");
    assert!(r.stderr().starts_with("error: "), "{}", r.stderr());
    // Failing health checks are errors too (1), not "refused".
    a.land_first_change();
    fs::remove_dir_all(a.dir.path().join(".ag/store/objects")).unwrap();
    a.ag(&["verify"]).code_is(1);
    a.ag(&["doctor"]).code_is(1);
}

#[test]
fn two_is_a_usage_error_from_the_argument_parser() {
    let a = Ag::new();
    a.ag(&["frobnicate"]).code_is(2);
    a.ag(&["snapshot"]).code_is(2); // --message is required
    a.ag(&["log", "--no-such-flag"]).code_is(2);
    a.ag(&["-q", "-v", "status"]).code_is(2);
    a.ag(&["completions", "cmd.exe"]).code_is(2);
    a.ag(&[]).code_is(2); // no command: the help text, on stderr
}

#[test]
fn three_is_blocked_and_the_same_command_resumes_once_the_thing_is_provided() {
    let a = Ag::new();
    a.land_first_change();
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    a.ag(&["policy", "set", "policy.json"]).ok();
    a.write("README.md", "# demo, second\n");
    let p = a.propose_as("user:alice", "second");
    let r = a.ag(&["integrate", &p]).code_is(3);
    assert!(r.stdout().contains("Blocked") && r.stdout().contains("the stream was not changed"), "{}", r.stdout());
    let before = a.ag(&["--json", "stream", "show", "main"]).ok().json();
    // `op resume` of a blocked operation is blocked again, and says so with the same code.
    let op = a.ag(&["--json", "op", "list"]).ok().json()[0]["operation_id"].as_str().unwrap().to_string();
    let r = a.ag(&["op", "resume", &op]).code_is(3);
    assert!(r.stdout().contains("approvals"), "{}", r.stdout());
    // The queue reports blocked entries with 3 as well.
    a.ag(&["queue", "add", &p]).ok();
    a.ag(&["queue", "run"]).code_is(3);
    assert_eq!(a.ag(&["--json", "stream", "show", "main"]).ok().json()["stream"]["state"], before["stream"]["state"]);
    // Provide the approval and run the same command again: now it lands.
    a.run_as("user:bob", &["proposal", "review", &p, "--approve"]).ok();
    a.ag(&["integrate", &p]).code_is(0);
}

#[test]
fn three_is_also_what_predict_returns_when_it_finds_overlapping_work() {
    let (a, ..) = two_competing_proposals();
    let r = a.ag(&["predict"]).code_is(3);
    assert!(r.stdout().contains("overlap"), "{}", r.stdout());
}

#[test]
fn four_is_refused_for_a_conflict_a_denial_an_invalid_request_and_a_failed_check() {
    let (a, pa, pb) = two_competing_proposals();
    // Preview and integrate report the conflict with 4 and leave the stream alone.
    a.ag(&["integrate", &pa]).code_is(0);
    let head = a.ag(&["--json", "stream", "show", "main"]).ok().json();
    let r = a.ag(&["preview", &pb]).code_is(4);
    assert!(r.stdout().contains("conflict: README.md"), "{}", r.stdout());
    let r = a.ag(&["integrate", &pb]).code_is(4);
    assert!(r.stdout().contains("Conflict"), "{}", r.stdout());
    assert_eq!(a.ag(&["--json", "stream", "show", "main"]).ok().json(), head, "a refusal must not move the stream");
    // sync leaves markers for a person and says so with 4.
    a.run_as("user:bob", &["sync", "--workspace", "bob", "--dir", a.dir.path().join(".bob-ws").to_str().unwrap()])
        .code_is(4);

    // A denial by policy: an agent may not integrate on its own.
    a.ag(&["agent", "register", "bot"]).ok();
    let who = a.ag(&["--json", "agent", "start", "bot"]).ok().json()["identity"].as_str().unwrap().to_string();
    a.write("docs.md", "agent was here\n");
    let pd = a.propose_as(&who, "docs");
    let r = a.run_as(&who, &["integrate", &pd]).code_is(4);
    assert!(r.stdout().contains("Denied"), "{}", r.stdout());

    // A failed check is recorded and exits 4.
    a.ag(&["validate", "keygen", "ci"]).ok();
    let pe = a.propose_as("user:alice", "e");
    let r =
        a.run_as("service:ci", &["validate", "run", &pe, "--check", "tests", "--key", "ci", "--", "false"]).code_is(4);
    assert!(r.stdout().contains("FAILED"), "{}", r.stdout());
    a.run_as("service:ci", &["validate", "run", &pe, "--check", "tests", "--key", "ci", "--", "true"]).code_is(0);
}

#[test]
fn the_remote_commands_use_the_same_codes() {
    let srv = Server::start();
    srv.create_repo("acme/demo");
    let alice = srv.user("user:alice", "acme/demo", Role::Writer);
    let bob = srv.user("user:bob", "acme/demo", Role::Reviewer);
    let carol = srv.user("user:carol", "acme/demo", Role::Integrator);
    let root_tok = "bootstrap-token-0123456789abcdef";
    let url = format!("{}/acme/demo", srv.url);

    let a = Ag::bare();
    let env = |t: &str| vec![("AG_TOKEN", t.to_string())];
    let run = |who: &str, tok: &str, args: &[&str]| {
        let e = env(tok);
        let pairs: Vec<(&str, &str)> = e.iter().map(|(k, v)| (*k, v.as_str())).collect();
        a.run_env(who, args, &pairs)
    };
    run("user:root", root_tok, &["init", "--namespace", "acme/demo"]).ok();
    run("user:root", root_tok, &["remote", "add", "origin", &url]).ok();
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    run("user:root", root_tok, &["remote", "policy", "main", "--set", "policy.json"]).ok();
    a.write("README.md", "# demo\n");
    run("user:alice", &alice, &["snapshot", "-m", "first"]).ok();
    let out = run("user:alice", &alice, &["push", "-t", "first"]).ok().stdout();
    let prop =
        out.split_whitespace().find(|w| w.starts_with("prop_")).unwrap().trim_end_matches(['.', ',']).to_string();

    run("user:carol", &carol, &["remote", "prepare", &prop]).code_is(3);
    run("user:carol", &carol, &["remote", "integrate", &prop]).code_is(3);
    run("user:bob", &bob, &["remote", "review", &prop, "--approve"]).ok();
    run("user:carol", &carol, &["remote", "prepare", &prop]).code_is(0);
    run("user:carol", &carol, &["remote", "integrate", &prop]).code_is(0);
    // An error from the server is 1 and names what happened.
    let r = run("user:carol", &carol, &["remote", "proposal", "prop_missing"]);
    assert_eq!(r.code(), 1, "{}", r.stderr());
}
