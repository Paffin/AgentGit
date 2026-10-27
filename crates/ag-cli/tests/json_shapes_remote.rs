//! Golden tests for the `--json` output of the commands that talk to an `ag-server` (see `json_shapes.rs`).
//! Regenerate with `AG_UPDATE_GOLDEN=1 cargo test -p agentgit-cli --test json_shapes_remote`.
#![allow(missing_docs)]

mod common;

use ag_server::authz::Role;
use common::golden::{Recorder, check_golden};
use common::{Ag, Server, run_in};

#[test]
fn the_json_output_of_every_remote_command_matches_the_documented_shape() {
    let srv = Server::start();
    srv.create_repo("acme/demo");
    let root = srv.user("user:root", "acme/demo", Role::Admin);
    let alice = srv.user("user:alice", "acme/demo", Role::Writer);
    let bob = srv.user("user:bob", "acme/demo", Role::Reviewer);
    let carol = srv.user("user:carol", "acme/demo", Role::Integrator);
    let url = format!("{}/acme/demo", srv.url);

    let a = Ag::bare();
    let as_user = |who: &str, tok: &str, args: &[&str]| a.run_env(who, args, &[("AG_TOKEN", tok)]);
    let mut r = Recorder::default();

    as_user("user:alice", &alice, &["init", "--namespace", "acme/demo"]).ok();
    as_user("user:alice", &alice, &["remote", "add", "origin", &url]).ok();
    r.rec("remote whoami", &as_user("user:alice", &alice, &["--json", "remote", "whoami"]).ok());
    r.rec("remote streams", &as_user("user:alice", &alice, &["--json", "remote", "streams"]).ok());
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    r.rec(
        "remote policy",
        &as_user("user:root", &root, &["--json", "remote", "policy", "main", "--set", "policy.json"]).ok(),
    );
    r.rec("remote policy", &as_user("user:alice", &alice, &["--json", "remote", "policy", "main"]).ok());

    // Two proposals: one is approved and integrated, one is refused for lack of approval and then closed.
    a.write("README.md", "# demo\n");
    as_user("user:alice", &alice, &["snapshot", "-m", "first"]).ok();
    let p1 = r.rec("push", &as_user("user:alice", &alice, &["--json", "push", "-t", "first"]).ok());
    let p1 = p1["proposal"]["id"].as_str().unwrap().to_string();
    r.rec("remote proposals", &as_user("user:alice", &alice, &["--json", "remote", "proposals"]).ok());
    r.rec("remote proposal", &as_user("user:alice", &alice, &["--json", "remote", "proposal", &p1]).ok());
    r.rec("remote prepare", &as_user("user:carol", &carol, &["--json", "remote", "prepare", &p1]).code_is(3));
    r.rec("remote explain", &as_user("user:carol", &carol, &["--json", "remote", "explain", &p1]).code_is(3));
    r.rec(
        "remote integrate#blocked",
        &as_user("user:carol", &carol, &["--json", "remote", "integrate", &p1]).code_is(3),
    );
    r.rec(
        "remote review",
        &as_user("user:bob", &bob, &["--json", "remote", "review", &p1, "--approve", "-m", "ok"]).ok(),
    );
    r.rec("remote prepare", &as_user("user:carol", &carol, &["--json", "remote", "prepare", &p1]).ok());
    r.rec("remote explain", &as_user("user:carol", &carol, &["--json", "remote", "explain", &p1]).ok());
    r.rec("remote integrate", &as_user("user:carol", &carol, &["--json", "remote", "integrate", &p1]).ok());
    r.rec("remote explain", &as_user("user:carol", &carol, &["--json", "remote", "explain", &p1]).code_is(4));
    a.write("two.txt", "2\n");
    as_user("user:alice", &alice, &["snapshot", "-m", "two"]).ok();
    let p2 = as_user("user:alice", &alice, &["--json", "push", "-t", "two"]).ok().json()["proposal"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    r.rec("remote close", &as_user("user:alice", &alice, &["--json", "remote", "close", &p2]).ok());

    // History and events on the server.
    r.rec("remote log", &as_user("user:alice", &alice, &["--json", "remote", "log"]).ok());
    r.rec("remote blame", &as_user("user:alice", &alice, &["--json", "remote", "blame", "main:README.md"]).ok());
    r.rec("remote events", &as_user("user:alice", &alice, &["--json", "remote", "events"]).ok());
    assert_eq!(as_user("user:alice", &alice, &["remote", "cat", "main:README.md"]).ok().stdout(), "# demo\n");

    // A clone, a fetch and a pull, and the status that follows them.
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("w");
    let cfg = tempfile::tempdir().unwrap();
    let env = [("AG_TOKEN", alice.as_str())];
    r.rec(
        "clone",
        &run_in(parent.path(), cfg.path(), "user:alice", &["--json", "clone", &url, dir.to_str().unwrap()], &env).ok(),
    );
    r.rec("fetch", &run_in(&dir, cfg.path(), "user:alice", &["--json", "fetch"], &env).ok());
    r.rec("pull", &run_in(&dir, cfg.path(), "user:alice", &["--json", "pull"], &env).ok());
    r.rec("status#remote", &run_in(&dir, cfg.path(), "user:alice", &["--json", "status", "--remote"], &env).ok());
    r.rec("status#remote", &run_in(&dir, cfg.path(), "user:alice", &["--json", "status"], &[]).ok());
    r.rec("doctor#remote", &run_in(&dir, cfg.path(), "user:alice", &["--json", "doctor"], &env).ok());

    // A saved login: the credential comes from a private file, as `ag-server dev` leaves it.
    let token_file = parent.path().join("token");
    std::fs::write(&token_file, &alice).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let login = ["--json", "login", srv.url.as_str(), "--token-file", token_file.to_str().unwrap()];
    r.rec("login", &run_in(parent.path(), cfg.path(), "user:alice", &login, &[]).ok());
    r.rec(
        "logout",
        &run_in(parent.path(), cfg.path(), "user:alice", &["--json", "logout", srv.url.as_str()], &[]).ok(),
    );

    check_golden("json-shapes-remote.json", &r.shapes, &[]);
}
