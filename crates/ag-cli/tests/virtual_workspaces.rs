//! Server-side workspaces through the real CLI, including guarded structural resolution.
#![allow(missing_docs)]
mod common;
use ag_server::authz::Role;
use common::{Ag, Server};
use serde_json::json;

#[test]
fn virtual_workspace_lifecycle_pins_structural_choices_and_preserves_other_files() {
    let server = Server::start();
    let repo = "acme/virtual-cli";
    server.create_repo(repo);
    let token = server.user("user:alice", repo, Role::Admin);
    let a = Ag::bare();
    let run = |args: &[&str]| a.run_env("user:alice", args, &[("AG_TOKEN", &token)]);
    let url = format!("{}/{repo}", server.url);
    run(&["init", "--namespace", repo]).ok();
    run(&["remote", "add", "origin", &url, "--token-env", "AG_TOKEN"]).ok();
    a.write("base.txt", "unchanged");
    run(&["snapshot", "-m", "base"]).ok();
    let base = run(&["--json", "push", "-t", "base"]).ok().json();
    run(&["remote", "integrate", base["proposal"]["id"].as_str().unwrap()]).ok();
    run(&["pull"]).ok();
    a.write("clash", "file");
    run(&["snapshot", "-m", "ours"]).ok();
    let ours = run(&["--json", "push", "-t", "ours"]).ok().json();
    let ours_head = ours["proposal"]["head"].as_str().unwrap();
    let ours_id = ours["proposal"]["id"].as_str().unwrap();
    let created = run(&["--json", "remote", "workspace", "create", "resolve", "--stream", "main", "--from", ours_head])
        .ok()
        .json();
    assert_eq!(created["head"], ours_head);
    assert_eq!(created["base"], ours_head);
    let listed = run(&["--json", "remote", "workspace", "list"]).ok().json();
    assert!(listed["workspaces"].as_array().unwrap().iter().any(|w| w["name"] == "resolve"));

    let b = Ag::bare();
    let other = |args: &[&str]| b.run_env("user:alice", args, &[("AG_TOKEN", &token)]);
    other(&["clone", &url, "."]).ok();
    b.write("clash/a", "a");
    b.write("clash/b", "b");
    b.write("unrelated", "keep");
    other(&["snapshot", "-m", "theirs"]).ok();
    let theirs = other(&["--json", "push", "-t", "theirs"]).ok().json();
    other(&["remote", "integrate", theirs["proposal"]["id"].as_str().unwrap()]).ok();
    other(&["pull"]).ok();
    let inspected = run(&["--json", "remote", "workspace", "sync", "resolve"]).code_is(4).json();
    assert_eq!(inspected["status"], "conflicted");
    assert_eq!(inspected["head"], ours_head);
    assert_eq!(
        inspected["conflicts"][0]["resolution_scope"],
        json!({"paths":["clash","clash/a","clash/b"],"complete":true})
    );
    let target = inspected["stream_head"].as_str().unwrap();
    a.write(
        "resolutions.json",
        &serde_json::to_string(&json!([
            {"path":"clash","op":"put","content":"ZmlsZQ=="},
            {"path":"clash/a","op":"delete"}, {"path":"clash/b","op":"delete"}
        ]))
        .unwrap(),
    );
    run(&["remote", "workspace", "sync", "resolve", "--resolutions", "resolutions.json"]).code_is(2);
    b.write("unrelated", "keep newer");
    other(&["snapshot", "-m", "advance"]).ok();
    let advanced = other(&["--json", "push", "-t", "advance"]).ok().json();
    other(&["remote", "integrate", advanced["proposal"]["id"].as_str().unwrap()]).ok();
    let stale = run(&[
        "remote",
        "workspace",
        "sync",
        "resolve",
        "--resolutions",
        "resolutions.json",
        "--expected-stream-head",
        target,
    ]);
    assert!(!stale.out.status.success());
    assert_eq!(run(&["--json", "remote", "workspace", "show", "resolve"]).ok().json()["head"], ours_head);
    let current = run(&["--json", "remote", "workspace", "sync", "resolve"]).code_is(4).json();
    let applied = run(&[
        "--json",
        "remote",
        "workspace",
        "sync",
        "resolve",
        "--resolutions",
        "resolutions.json",
        "--expected-stream-head",
        current["stream_head"].as_str().unwrap(),
    ])
    .ok()
    .json();
    assert_eq!(applied["status"], "merged");
    let result = applied["head"].as_str().unwrap();
    assert_eq!(run(&["remote", "cat", &format!("{result}:unrelated")]).ok().stdout(), "keep newer");
    assert_eq!(run(&["remote", "cat", &format!("{result}:clash")]).ok().stdout(), "file");
    assert_eq!(run(&["--json", "remote", "proposal", ours_id]).ok().json()["proposal"]["head"], ours_head);
    other(&["pull"]).ok();
    for index in 0..100 {
        b.write(&format!("clash/wide-{index}"), "wide");
    }
    other(&["snapshot", "-m", "wide directory"]).ok();
    let wide = other(&["--json", "push", "-t", "wide"]).ok().json();
    other(&["remote", "integrate", wide["proposal"]["id"].as_str().unwrap()]).ok();
    run(&["remote", "workspace", "create", "wide", "--from", ours_head]).ok();
    let incomplete = run(&["--json", "remote", "workspace", "sync", "wide"]).code_is(4).json();
    assert_eq!(incomplete["conflicts"][0]["resolution_scope"]["complete"], false);
    assert_eq!(run(&["--json", "remote", "workspace", "show", "wide"]).ok().json()["head"], ours_head);
    run(&["remote", "workspace", "delete", "wide"]).ok();
    run(&["remote", "workspace", "delete", "resolve"]).ok();
    assert!(!run(&["remote", "workspace", "show", "resolve"]).out.status.success());
}
