//! `ag doctor` and the one-screen `ag status`, against a scratch repository and an in-process server.
#![allow(missing_docs)]

mod common;

use std::fs;

use ag_server::authz::Role;
use common::{Ag, Server, run_in};
use serde_json::Value;

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no check '{id}' in {report}"))
}

#[test]
fn doctor_on_a_healthy_repository_passes_and_says_what_it_looked_at() {
    let a = Ag::new();
    a.land_first_change();
    let r = a.ag(&["doctor"]).ok();
    let out = r.stdout();
    for needle in ["version", "identity", "repository", "integrity", "keys", "operations", "remotes"] {
        assert!(out.contains(needle), "missing '{needle}' in:\n{out}");
    }
    assert!(out.contains("user:alice (from AG_IDENTITY)"), "{out}");
    assert!(out.contains("0 failed"), "{out}");

    let j = a.ag(&["--json", "doctor"]).ok().json();
    assert_eq!(j["ok"], true);
    assert_eq!(j["summary"]["fail"], 0);
    assert_eq!(check(&j, "repository")["status"], "ok");
    assert_eq!(check(&j, "integrity")["status"], "ok");
    assert_eq!(check(&j, "remotes")["status"], "skip");
    // Every check carries the same keys, so a consumer can rely on them.
    for c in j["checks"].as_array().unwrap() {
        assert!(c["id"].is_string() && c["status"].is_string() && c["summary"].is_string() && c.get("hint").is_some());
    }
}

#[test]
fn doctor_outside_a_repository_still_checks_the_installation_and_does_not_fail() {
    let a = Ag::bare();
    let j = a.ag(&["--json", "doctor"]).ok().json();
    assert_eq!(check(&j, "version")["status"], "ok");
    assert_eq!(check(&j, "repository")["status"], "skip");
    assert!(check(&j, "repository")["hint"].as_str().unwrap().contains("ag init"));
    // ... but naming a directory that is not a repository is a failure.
    let r = a.ag(&["-C", a.dir.path().join("nope").to_str().unwrap(), "doctor"]).code_is(1);
    assert!(r.stdout().contains("FAIL") && r.stdout().contains("not an AgentGit repository"), "{}", r.stdout());
}

#[test]
fn doctor_flags_an_invalid_identity_and_a_damaged_user_file() {
    let a = Ag::new();
    let r = a.run_as("bogus", &["--json", "doctor"]).code_is(1);
    let j = r.json();
    assert_eq!(check(&j, "identity")["status"], "fail");
    assert!(check(&j, "identity")["summary"].as_str().unwrap().contains("invalid identity"));
    fs::write(a.cfg.path().join("config.json"), "{ nope").unwrap();
    let j = a.ag(&["--json", "doctor"]).code_is(1).json();
    assert_eq!(check(&j, "user_config")["status"], "fail");
    assert!(check(&j, "user_config")["summary"].as_str().unwrap().contains("not valid JSON"));
}

// Unix only: Windows has no mode bits (keys get an owner-only ACL from `ag-crypto::private_file`, which `std` cannot
// read back), so `ag doctor` has no permission check there; the test of that ACL is in ag-crypto.
#[cfg(unix)]
#[test]
fn doctor_warns_about_keys_other_users_can_read_and_strict_turns_warnings_into_failures() {
    use std::os::unix::fs::PermissionsExt;

    let a = Ag::new();
    a.ag(&["agent", "register", "bot"]).ok();
    assert_eq!(check(&a.ag(&["--json", "doctor"]).ok().json(), "keys")["status"], "ok");
    let key = a.dir.path().join(".ag/keys/bot.json");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let r = a.ag(&["--json", "doctor"]).ok();
    let j = r.json();
    assert_eq!(check(&j, "keys")["status"], "warn");
    assert!(check(&j, "keys")["hint"].as_str().unwrap().contains("chmod 600"));
    assert_eq!(j["summary"]["warn"], 1);
    let strict = a.ag(&["doctor", "--strict"]).code_is(1);
    assert!(strict.stdout().contains("warn"), "{}", strict.stdout());
}

#[test]
fn doctor_reports_a_registered_agent_whose_key_file_is_gone() {
    let a = Ag::new();
    a.ag(&["agent", "register", "bot"]).ok();
    assert_eq!(check(&a.ag(&["--json", "doctor"]).ok().json(), "keys")["status"], "ok");
    fs::remove_file(a.dir.path().join(".ag/keys/bot.json")).unwrap();
    let j = a.ag(&["--json", "doctor"]).ok().json();
    assert_eq!(check(&j, "keys")["status"], "warn");
    assert!(check(&j, "keys")["summary"].as_str().unwrap().contains("bot"));
}

#[test]
fn doctor_fails_when_a_stream_head_object_is_gone_and_points_at_verify() {
    let a = Ag::new();
    a.land_first_change();
    assert_eq!(check(&a.ag(&["--json", "doctor"]).ok().json(), "integrity")["status"], "ok");
    // Remove every stored object: the head changeset can no longer be read.
    let objects = a.dir.path().join(".ag/store/objects");
    fn walk(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, f) } else { f(&p) }
        }
    }
    walk(&objects, &mut |p| fs::remove_file(p).unwrap());
    let r = a.ag(&["--json", "doctor"]).code_is(1);
    let j = r.json();
    assert_eq!(j["ok"], false);
    let c = check(&j, "integrity");
    assert_eq!(c["status"], "fail");
    assert!(c["hint"].as_str().unwrap().contains("ag verify"), "{c}");
    // The human output ends with what to do.
    assert!(a.ag(&["doctor"]).code_is(1).stdout().contains("Fix the failed checks"));
}

#[test]
fn doctor_reports_a_remote_credential_by_what_the_server_says() {
    let srv = Server::start();
    srv.create_repo("acme/demo");
    let good = srv.user("user:alice", "acme/demo", Role::Writer);
    let short = srv.user_ttl("user:alice", "acme/demo", Role::Writer, 3_600_000);
    let url = format!("{}/acme/demo", srv.url);
    let a = Ag::new();
    a.ag(&["remote", "add", "origin", &url]).ok();

    // No credential at all: a failure that says how to provide one, without touching the network.
    let j = a.ag(&["--json", "doctor"]).code_is(1).json();
    let c = check(&j, "remote.origin");
    assert_eq!(c["status"], "fail");
    assert!(c["summary"].as_str().unwrap().contains("no credential"), "{c}");
    assert!(c["hint"].as_str().unwrap().contains("AG_TOKEN"));

    // A good, long-lived credential.
    let j = a.run_env("user:alice", &["--json", "doctor"], &[("AG_TOKEN", &good)]).ok().json();
    let c = check(&j, "remote.origin");
    assert_eq!(c["status"], "ok", "{c}");
    let s = c["summary"].as_str().unwrap();
    for needle in ["authenticated as user:alice", "writer", "expires in 29d", "repository visible", "agwp.v1"] {
        assert!(s.contains(needle), "'{needle}' missing from: {s}");
    }

    // A credential that is about to expire is a warning with a remedy.
    let j = a.run_env("user:alice", &["--json", "doctor"], &[("AG_TOKEN", &short)]).ok().json();
    let c = check(&j, "remote.origin");
    assert_eq!(c["status"], "warn", "{c}");
    assert!(
        c["summary"].as_str().unwrap().contains("expires in 59m")
            || c["summary"].as_str().unwrap().contains("expires in 1h"),
        "{c}"
    );
    assert!(c["hint"].as_str().unwrap().contains("fresh credential"));

    // The wrong secret is rejected, and the message says so rather than "connection error".
    let j =
        a.run_env("user:alice", &["--json", "doctor"], &[("AG_TOKEN", "agt_0000000000000000_wrong")]).code_is(1).json();
    let c = check(&j, "remote.origin");
    assert_eq!(c["status"], "fail");
    assert!(c["summary"].as_str().unwrap().contains("rejected the credential"), "{c}");

    // A repository the credential cannot see.
    a.ag(&["remote", "add", "other", &format!("{}/acme/missing", srv.url)]).ok();
    let j = a.run_env("user:alice", &["--json", "doctor"], &[("AG_TOKEN", &good)]).code_is(1).json();
    let c = check(&j, "remote.other");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["summary"].as_str().unwrap().contains("acme/missing"), "{c}");
    a.ag(&["remote", "remove", "other"]).ok();

    // --offline checks that a credential exists but does not use it.
    let j = a
        .run_env("user:alice", &["--json", "doctor", "--offline"], &[("AG_TOKEN", "agt_0000000000000000_wrong")])
        .ok()
        .json();
    assert_eq!(check(&j, "remote.origin")["status"], "skip");
}

#[test]
fn doctor_reports_an_unreachable_server_with_a_hint_and_respects_the_timeout() {
    let a = Ag::new();
    // Nothing listens on port 9 (discard) of loopback in the test environment.
    a.ag(&["remote", "add", "origin", "http://127.0.0.1:9/acme/demo"]).ok();
    let started = std::time::Instant::now();
    let j =
        a.run_env("user:alice", &["--json", "doctor", "--timeout", "3"], &[("AG_TOKEN", "agt_x")]).code_is(1).json();
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let c = check(&j, "remote.origin");
    assert_eq!(c["status"], "fail");
    assert!(c["summary"].as_str().unwrap().contains("cannot reach"), "{c}");
    assert!(c["hint"].as_str().unwrap().contains("ag-server"));
}

#[test]
fn doctor_notices_a_conflicted_sync_that_is_waiting_for_a_person() {
    let a = Ag::new();
    a.land_first_change();
    let bob_dir = a.dir.path().join("../bob-ws");
    let bob_dir = bob_dir.to_str().unwrap().to_string();
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", &bob_dir]).ok();
    assert!(
        a.ag(&["--json", "doctor"]).ok().json()["checks"].as_array().unwrap().iter().all(|c| c["id"] != "workspace")
    );
    // Bob lands a change to the first line of the README; alice edits the same line.
    std::fs::write(std::path::Path::new(&bob_dir).join("README.md"), "# bob\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob", "--workspace", "bob"]).ok();
    let p = a.run_as("user:bob", &["--json", "propose", "@bob", "-t", "bob"]).ok().json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    a.ag(&["integrate", &p]).ok();
    a.write("README.md", "# alice\n");
    a.ag(&["snapshot", "-m", "alice"]).ok();
    a.ag(&["sync"]).code_is(4);
    let j = a.ag(&["--json", "doctor"]).ok().json();
    let c = check(&j, "workspace");
    assert_eq!(c["status"], "warn");
    assert!(c["summary"].as_str().unwrap().contains("default"), "{c}");
    assert!(c["hint"].as_str().unwrap().contains("ag snapshot"));
    let _ = std::fs::remove_dir_all(&bob_dir);
}

#[test]
fn status_summarizes_the_workspace_proposals_leases_and_the_next_step() {
    let a = Ag::new();
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["relation"], "empty");
    assert_eq!(j["summary"]["identity"], "user:alice");
    assert_eq!(j["summary"]["identity_source"], "env");

    a.write("README.md", "# demo\n");
    let out = a.ag(&["status"]).ok().stdout();
    assert!(out.contains("added     README.md") && out.contains("next:") && out.contains("ag snapshot"), "{out}");

    a.ag(&["snapshot", "-m", "first"]).ok();
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["relation"], "ahead");
    assert_eq!(j["summary"]["ahead"], 1);
    assert_eq!(j["summary"]["unpublished"], 1);
    assert!(j["summary"]["next"].as_str().unwrap().contains("ag propose"));

    let p = a.ag(&["--json", "propose", "-t", "first"]).ok().json()["id"].as_str().unwrap().to_string();
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["unpublished"], 0);
    assert_eq!(j["summary"]["proposals"]["open"], 1);
    assert_eq!(j["summary"]["proposals"]["mine"][0]["id"], p.as_str());
    assert_eq!(j["summary"]["proposals"]["mine"][0]["is_workspace_head"], true);
    assert!(j["summary"]["next"].as_str().unwrap().contains(&format!("ag integrate {p}")));

    // Someone else's proposal is counted as open but is not "mine".
    a.write("other.txt", "x\n");
    let q = a.propose_as("user:bob", "bob's");
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["proposals"]["open"], 2);
    assert_eq!(j["summary"]["proposals"]["mine"].as_array().unwrap().len(), 1);
    let text = a.ag(&["status"]).ok().stdout();
    assert!(text.contains("2 open, 1 yours"), "{text}");
    assert!(!text.contains(&q), "another person's proposal is not listed as yours");

    a.ag(&["integrate", &p]).ok();
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["relation"], "ahead", "bob's snapshot is on top of alice's");

    // Leases and tasks.
    a.ag(&["lease", "acquire", "src/**", "--exclusive"]).ok();
    let j = a.ag(&["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["leases"]["mine"][0]["scope"], "src/**");
    assert_eq!(j["summary"]["leases"]["mine"][0]["exclusive"], true);
    let text = a.ag(&["status"]).ok().stdout();
    assert!(text.contains("1 held by you") && text.contains("token 1"), "{text}");
}

#[test]
fn status_shows_the_task_of_an_agent_run() {
    let a = Ag::new();
    a.ag(&["agent", "register", "bot"]).ok();
    let task = a.ag(&["--json", "task", "create", "Fix the parser"]).ok().json()["id"].as_str().unwrap().to_string();
    let run = a.ag(&["--json", "agent", "start", "bot", "--task", &task]).ok().json();
    let who = run["identity"].as_str().unwrap().to_string();
    let j = a.run_as(&who, &["--json", "status"]).ok().json();
    assert_eq!(j["summary"]["task"]["id"], task.as_str());
    assert_eq!(j["summary"]["task"]["title"], "Fix the parser");
    assert!(a.run_as(&who, &["status"]).ok().stdout().contains("Fix the parser"));
    // A human with no task of their own sees none.
    assert!(a.ag(&["--json", "status"]).ok().json()["summary"]["task"].is_null());
}

#[test]
fn status_tracks_how_the_local_stream_relates_to_a_remote() {
    let srv = Server::start();
    srv.create_repo("acme/demo");
    let tok = srv.user("user:alice", "acme/demo", Role::Admin);
    let url = format!("{}/acme/demo", srv.url);

    // Alice publishes a first change to the server through a second working copy.
    let seed = Ag::new();
    seed.ag(&["remote", "add", "origin", &url, "--token-env", "AG_TOKEN"]).ok();
    seed.write("README.md", "# demo\n");
    seed.run_env("user:alice", &["snapshot", "-m", "seed"], &[]).ok();
    let out = seed.run_env("user:alice", &["push", "-t", "seed"], &[("AG_TOKEN", &tok)]).ok().stdout();
    let prop =
        out.split_whitespace().find(|w| w.starts_with("prop_")).unwrap().trim_end_matches(['.', ',']).to_string();
    seed.run_env("user:alice", &["remote", "integrate", &prop], &[("AG_TOKEN", &tok)]).ok();

    // A fresh clone is up to date.
    let parent = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let dir = parent.path().join("w");
    run_in(parent.path(), cfg.path(), "user:alice", &["clone", &url, dir.to_str().unwrap()], &[("AG_TOKEN", &tok)])
        .ok();
    let j = run_in(&dir, cfg.path(), "user:alice", &["--json", "status"], &[]).ok().json();
    let r = &j["summary"]["remotes"][0];
    assert_eq!(r["name"], "origin");
    assert_eq!(r["state"], "up_to_date", "{j}");
    let text = run_in(&dir, cfg.path(), "user:alice", &["status"], &[]).ok().stdout();
    assert!(text.contains("remote") && text.contains("up to date as of the last fetch"), "{text}");

    // A second change lands on the server; a fetch (not a pull) records it.
    seed.write("two.txt", "2\n");
    seed.run_env("user:alice", &["snapshot", "-m", "two"], &[]).ok();
    // With --quiet, `push` prints just the proposal id, like `propose` (the tutorials capture it with `$(...)`).
    let out = seed.run_env("user:alice", &["-q", "push", "-t", "two"], &[("AG_TOKEN", &tok)]).ok().stdout();
    let prop = out.trim().to_string();
    assert!(prop.starts_with("prop_") && !prop.contains(char::is_whitespace), "a bare proposal id: {out:?}");
    seed.run_env("user:alice", &["remote", "integrate", &prop], &[("AG_TOKEN", &tok)]).ok();
    run_in(&dir, cfg.path(), "user:alice", &["fetch"], &[("AG_TOKEN", &tok)]).ok();
    let j = run_in(&dir, cfg.path(), "user:alice", &["--json", "status"], &[]).ok().json();
    let r = &j["summary"]["remotes"][0];
    assert_eq!(r["state"], "fetched_not_pulled", "{j}");
    assert_eq!(r["behind"], 1);
    assert!(j["summary"]["next"].as_str().unwrap().contains("ag pull"));

    // --remote asks the server for its head and compares it with the last fetch.
    let j =
        run_in(&dir, cfg.path(), "user:alice", &["--json", "status", "--remote"], &[("AG_TOKEN", &tok)]).ok().json();
    assert_eq!(j["summary"]["remotes"][0]["server_head"], j["summary"]["remotes"][0]["fetched_head"]);
    run_in(&dir, cfg.path(), "user:alice", &["pull"], &[("AG_TOKEN", &tok)]).ok();
    let j = run_in(&dir, cfg.path(), "user:alice", &["--json", "status"], &[]).ok().json();
    assert_eq!(j["summary"]["remotes"][0]["state"], "up_to_date");
}

#[test]
fn doctor_reports_the_derive_status_and_dangling_references() {
    let a = Ag::new();
    a.land_first_change();
    let j = a.ag(&["--json", "doctor"]).ok().json();
    assert_eq!(check(&j, "derive")["status"], "ok");
    assert_eq!(check(&j, "references")["status"], "ok");

    // A pin and an open proposal that point at a changeset that never existed, and a derive event that failed.
    a.ag(&["pin", "keep", "main"]).ok();
    a.write("src/more.rs", "pub fn two() -> u32 { 2 }\n");
    let p = a.propose_as("user:alice", "more");
    let ghost = "ab".repeat(32);
    {
        let db = rusqlite::Connection::open(a.root().join(".ag/meta.db")).unwrap();
        db.execute("UPDATE pins SET changeset = ?1", [&ghost]).unwrap();
        db.execute("UPDATE proposals SET head = ?1 WHERE id = ?2", [&ghost, &p]).unwrap();
        db.execute(
            "INSERT INTO meta (key, value) VALUES ('derive.last_error', 'caching the tree: disk full')
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [],
        )
        .unwrap();
    }
    let r = a.ag(&["--json", "doctor"]).code_is(1);
    let j = r.json();
    let refs = check(&j, "references");
    assert_eq!(refs["status"], "fail", "{j}");
    let summary = refs["summary"].as_str().unwrap();
    assert!(summary.contains("1 dangling_pin") && summary.contains("1 dangling_proposal_head"), "{summary}");
    assert!(refs["hint"].as_str().unwrap().contains("garbage collection"));
    let derive = check(&j, "derive");
    assert_eq!(derive["status"], "warn", "{j}");
    assert!(derive["summary"].as_str().unwrap().contains("disk full"));
}
