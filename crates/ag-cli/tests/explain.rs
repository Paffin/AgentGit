//! `ag explain` and the hints under a refusal, against the real binary: each reason is put in words with commands
//! that, run in order, really do land the change.
#![allow(missing_docs)]

mod common;

use common::Ag;

fn codes(j: &serde_json::Value) -> Vec<String> {
    j["explanations"].as_array().unwrap().iter().map(|e| e["code"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_blocked_proposal_is_explained_and_the_printed_commands_land_it() {
    let a = Ag::new();
    a.land_first_change();
    a.ag(&["policy", "preset", "apply", "team", "--ci-key", "ci"]).ok();
    a.write("README.md", "# demo, changed\n");
    let p = a.propose_as("user:alice", "change");

    let j = a.ag(&["--json", "explain", &p]).code_is(3).json();
    assert_eq!(j["standing"], "blocked");
    assert_eq!(codes(&j), ["missing_validation", "insufficient_approvals"]);
    let text = a.ag(&["explain", &p]).code_is(3).stdout();
    assert!(text.contains("The check 'tests' has not passed on this exact change"), "{text}");
    assert!(text.contains("Why:") && text.contains("Next:"), "{text}");

    // Do what the explanation says, with its own command lines.
    let next: Vec<&str> = j["next"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert_eq!(next[0], format!("ag validate run {p} --check tests --key <issuer-key> -- <command>"));
    a.ag(&["validate", "run", &p, "--check", "tests", "--key", "ci", "--", "true"]).ok();
    let j = a.ag(&["--json", "explain", &p]).code_is(3).json();
    assert_eq!(codes(&j), ["insufficient_approvals"], "the check no longer stands in the way");
    assert_eq!(j["next"][0], format!("ag proposal review {p} --approve"));
    a.run_as("user:bob", &["proposal", "review", &p, "--approve"]).ok();
    let j = a.ag(&["--json", "explain", &p]).ok().json();
    assert_eq!(j["standing"], "ready");
    assert_eq!(j["next"], serde_json::json!([]));
    assert!(a.ag(&["explain", &p]).ok().stdout().contains(&format!("ag integrate {p}")));
    a.ag(&["integrate", &p]).ok();
    // A landed proposal has nothing to fix.
    assert!(a.ag(&["explain", &p]).code_is(4).stdout().contains("already landed"));
}

#[test]
fn the_refusal_itself_carries_the_same_commands() {
    let a = Ag::new();
    a.land_first_change();
    a.write("p.json", r#"{"approvals": {"min": 1}}"#);
    a.ag(&["policy", "set", "p.json"]).ok();
    a.write("README.md", "# demo, again\n");
    let p = a.propose_as("user:alice", "again");
    let out = a.ag(&["integrate", &p]).code_is(3).stdout();
    assert!(out.contains(&format!("  ag proposal review {p} --approve")), "{out}");
    assert!(out.contains(&format!("`ag explain {p}` says why")), "{out}");
}

#[test]
fn a_conflict_is_explained_with_the_steps_that_resolve_it() {
    let a = Ag::new();
    a.land_first_change();
    let bob_dir = a.root().join(".bob-ws");
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", bob_dir.to_str().unwrap()]).ok();
    std::fs::write(bob_dir.join("README.md"), "# bob's title\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob", "--workspace", "bob"]).ok();
    let pb = a.run_as("user:bob", &["--json", "propose", "@bob", "-t", "bob"]).ok().json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    a.write("README.md", "# alice's title\n");
    let pa = a.propose_as("user:alice", "alice");
    a.ag(&["integrate", &pa]).ok();

    let j = a.run_as("user:bob", &["--json", "explain", &pb]).code_is(4).json();
    assert_eq!(j["standing"], "refused");
    assert_eq!(codes(&j), ["content"]);
    assert_eq!(j["explanations"][0]["kind"], "conflict");
    assert_eq!(j["next"][0], "ag sync");
    let out = a.run_as("user:bob", &["integrate", &pb]).code_is(4).stdout();
    assert!(out.contains("Fix: `ag sync`") && out.contains(&format!("`ag proposal update {pb}`")), "{out}");
}

#[test]
fn a_denial_says_what_to_do_and_without_an_argument_the_newest_own_proposal_is_used() {
    let a = Ag::new();
    a.land_first_change();
    a.write("p.json", r#"{"max_changed_files": 1}"#);
    a.ag(&["policy", "set", "p.json"]).ok();
    a.write("a.txt", "a\n");
    a.write("b.txt", "b\n");
    let p = a.propose_as("user:alice", "two files");
    let denied = a.ag(&["integrate", &p]).code_is(4).stdout();
    assert!(denied.contains("What to do:"), "{denied}");
    let j = a.ag(&["--json", "explain"]).code_is(4).json();
    assert_eq!(j["proposal"], p.as_str());
    assert_eq!(codes(&j), ["too_many_changed_files"]);
    assert_eq!(j["explanations"][0]["kind"], "denied");
}

#[test]
fn nothing_to_explain_is_an_error_with_the_way_out() {
    let a = Ag::new();
    let r = a.ag(&["explain"]).code_is(1);
    assert!(r.stderr().contains("no open proposal") && r.stderr().contains("ag propose"), "{}", r.stderr());
}
