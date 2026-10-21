//! `ag policy preset`, `ag policy explain` and `ag init --preset` against the real binary: the stored document is
//! the preset, the explanation is read from it, and the policy the preset stores really governs a landing.
#![allow(missing_docs)]

mod common;

use ag_policy::Policy;
use common::Ag;

#[test]
fn the_catalogue_lists_every_preset_and_works_outside_a_repository() {
    let a = Ag::bare();
    let r = a.ag(&["--json", "policy", "preset", "list"]).ok().json();
    let names: Vec<&str> = r.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["solo", "team", "agents", "strict"]);
    let strict = &r[3];
    assert_eq!(strict["needs_ci_issuer"], true);
    assert_eq!(r[0]["needs_ci_issuer"], false);
    let text = a.ag(&["policy", "preset", "list"]).ok().stdout();
    assert!(text.contains("agents") && text.contains("needs --ci-key or --issuer"), "{text}");
}

#[test]
fn show_prints_a_document_the_real_parser_accepts_and_the_sentences_read_from_it() {
    let a = Ag::bare();
    for name in ["solo", "team", "agents"] {
        let r = a.ag(&["--json", "policy", "preset", "show", name]).ok().json();
        let policy = Policy::parse(&r["policy"].to_string()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let expected = ag_policy::explain::explain(&policy);
        assert_eq!(r["explanation"], serde_json::to_value(&expected).unwrap(), "{name}");
        assert!(!expected.is_empty());
    }
    // The administrator defaults to the acting identity and can be named.
    let team = a.ag(&["--json", "policy", "preset", "show", "team"]).ok().json();
    assert_eq!(team["policy"]["admins"], serde_json::json!(["user:alice"]));
    let named = a.ag(&["--json", "policy", "preset", "show", "team", "--admin", "user:root"]).ok().json();
    assert_eq!(named["policy"]["admins"], serde_json::json!(["user:root"]));
    // Solo has no administrator list.
    let solo = a.ag(&["--json", "policy", "preset", "show", "solo"]).ok().json();
    assert_eq!(solo["policy"]["admins"], serde_json::json!([]));
}

#[test]
fn a_preset_that_needs_a_check_says_how_to_provide_the_issuer() {
    let a = Ag::bare();
    let r = a.ag(&["policy", "preset", "show", "strict"]).code_is(1);
    assert!(r.stderr().contains("--ci-key ci") && r.stderr().contains("--issuer service:ci="), "{}", r.stderr());
    let key = ag_crypto::KeyPair::from_seed([5; 32]).public().to_hex();
    let ok =
        a.ag(&["--json", "policy", "preset", "show", "strict", "--issuer", &format!("service:ci={key}")]).ok().json();
    assert_eq!(ok["policy"]["trusted_issuers"][0]["identity"], "service:ci");
    assert_eq!(ok["policy"]["trusted_issuers"][0]["public_key"], key.as_str());
    let bad = a.ag(&["policy", "preset", "show", "strict", "--issuer", "service:ci=not-a-key"]).code_is(1);
    assert!(bad.stderr().contains("--issuer") && bad.stderr().contains("public key"), "{}", bad.stderr());
    a.ag(&["policy", "preset", "show", "strict", "--issuer", "service:ci"]).code_is(1);
    a.ag(&["policy", "preset", "show", "lax"]).code_is(1);
}

#[test]
fn init_with_a_preset_stores_it_and_a_typo_leaves_nothing_behind() {
    let a = Ag::bare();
    let r = a.ag(&["init", "--namespace", "acme/p", "--preset", "nonsense"]).code_is(1);
    assert!(r.stderr().contains("solo, team, agents, strict"), "{}", r.stderr());
    assert!(!a.root().join(".ag").exists(), "a refused preset must not leave a repository");
    a.ag(&["init", "--preset", "strict"]).code_is(1);
    assert!(!a.root().join(".ag").exists());
    a.ag(&["init", "--admin", "user:bob"]).code_is(1); // flags that belong to a preset, without one

    let init = a.ag(&["--json", "init", "--namespace", "acme/p", "--preset", "team"]).ok().json();
    assert_eq!(init["preset"], "team");
    assert_eq!(init["policy_version"], 1);
    let shown = a.ag(&["--json", "policy", "show"]).ok().json();
    let preset = a.ag(&["--json", "policy", "preset", "show", "team"]).ok().json();
    assert_eq!(shown["policy"], preset["policy"], "what is stored is what the preset shows");
}

#[test]
fn applying_a_preset_changes_what_may_land() {
    let a = Ag::new();
    a.land_first_change();
    a.write("README.md", "# demo, second\n");
    let p = a.propose_as("user:alice", "second");
    // Without a policy a person lands their own change.
    // A fresh proposal under the team preset needs somebody else.
    a.ag(&["policy", "preset", "apply", "team"]).ok();
    let r = a.ag(&["--json", "integrate", &p]).code_is(3).json();
    assert_eq!(r["outcome"]["blockers"][0]["code"], "insufficient_approvals");
    a.run_as("user:bob", &["proposal", "review", &p, "--approve"]).ok();
    a.ag(&["integrate", &p]).ok();
    // Applying the same preset again is not a new version.
    let again = a.ag(&["--json", "policy", "preset", "apply", "team"]).ok().json();
    assert_eq!(again["version"], 1);
    assert!(a.ag(&["policy", "preset", "apply", "team"]).ok().stdout().contains("unchanged"));
    // Only an administrator may replace it: the preset named alice.
    a.run_as("user:mallory", &["policy", "preset", "apply", "solo"]).code_is(1);
}

#[test]
fn a_ci_key_makes_the_check_required_and_the_key_signs_it() {
    let a = Ag::new();
    a.land_first_change();
    a.ag(&["policy", "preset", "apply", "team", "--ci-key", "ci"]).ok();
    assert!(a.root().join(".ag/keys/issuer-ci.json").is_file());
    let shown = a.ag(&["--json", "policy", "show"]).ok().json();
    assert_eq!(shown["policy"]["required_validations"][0]["check"], "tests");
    assert_eq!(shown["policy"]["trusted_issuers"][0]["identity"], "service:ci");

    a.write("README.md", "# demo, checked\n");
    let p = a.propose_as("user:alice", "checked");
    a.run_as("user:bob", &["proposal", "review", &p, "--approve"]).ok();
    let blocked = a.ag(&["--json", "integrate", &p]).code_is(3).json();
    assert_eq!(blocked["outcome"]["blockers"][0]["code"], "missing_validation");
    a.ag(&["validate", "run", &p, "--check", "tests", "--key", "ci", "--", "true"]).ok();
    a.ag(&["integrate", &p]).ok();
}

#[test]
fn explain_reads_the_policy_in_force_or_a_file() {
    let a = Ag::new();
    // The built-in default.
    let r = a.ag(&["policy", "explain"]).ok().stdout();
    assert!(r.contains("built-in default"), "{r}");
    assert!(r.contains("cannot land a change itself"), "{r}");
    // A document with numbers: the sentences follow them.
    a.write("p.json", r#"{"approvals": {"min": 3}, "max_changed_files": 7}"#);
    let r = a.ag(&["policy", "explain", "p.json"]).ok().stdout();
    assert!(r.contains("needs 3 approvals") && r.contains("more than 7 files"), "{r}");
    // `current` is the stored one.
    a.ag(&["policy", "set", "p.json"]).ok();
    let j = a.ag(&["--json", "policy", "explain", "current"]).ok().json();
    assert_eq!(j["version"], 1);
    let texts: Vec<&str> = j["explanation"].as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap()).collect();
    assert!(texts.iter().any(|t| t.contains("needs 3 approvals")), "{texts:?}");
    // A file that is not a policy is refused with the parser's reason, and works without a repository.
    a.write("bad.json", r#"{"requird_validations": []}"#);
    let bad = a.ag(&["policy", "explain", "bad.json"]).code_is(1);
    assert!(bad.stderr().contains("unknown field"), "{}", bad.stderr());
    let outside = Ag::bare();
    outside.write("p.json", r#"{"approvals": {"min": 2}}"#);
    assert!(outside.ag(&["policy", "explain", "p.json"]).ok().stdout().contains("needs 2 approvals"));
}

#[test]
fn a_preset_can_be_stored_on_a_server_and_comes_back_as_the_same_document() {
    use ag_server::authz::Role;
    let server = common::Server::start();
    server.create_repo("acme/web");
    let token = server.user("user:alice", "acme/web", Role::Admin);
    let work = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let env = [("AG_TOKEN", token.as_str())];
    let url = format!("{}/acme/web", server.url);
    common::run_in(work.path(), cfg.path(), "user:alice", &["clone", &url, "web"], &env).ok();
    let clone = work.path().join("web");
    let run = |args: &[&str]| common::run_in(&clone, cfg.path(), "user:alice", args, &env);

    run(&["remote", "policy", "main", "--preset", "agents"]).ok();
    let stored = run(&["--json", "remote", "policy", "main"]).ok().json();
    let doc = Policy::parse(stored["document"].as_str().unwrap()).unwrap();
    let local = run(&["--json", "policy", "preset", "show", "agents"]).ok().json();
    assert_eq!(serde_json::to_value(&doc).unwrap(), local["policy"], "the server holds what the preset shows");
    assert_eq!(doc.admins, ["user:alice"]);
    // A preset and a file are two ways to say the same thing: not both.
    run(&["remote", "policy", "main", "--preset", "team", "--set", "x.json"]).code_is(2);
    // The check of `strict` needs an issuer here as anywhere else.
    let r = run(&["remote", "policy", "main", "--preset", "strict"]).code_is(1);
    assert!(r.stderr().contains("trusted CI issuer"), "{}", r.stderr());
}
