//! Comment drafts are private notes for writers, including after proposal closure.
#![allow(missing_docs)]
mod common;
use ag_server::authz::Role;
use common::{Ag, Server};

#[test]
fn writers_keep_private_versioned_comment_drafts_without_posting_or_touching_review_drafts() {
    let server = Server::start();
    let repo = "acme/comment-drafts";
    server.create_repo(repo);
    let alice = server.user("user:alice", repo, Role::Writer);
    let bob = server.user("user:bob", repo, Role::Writer);
    let reader = server.user("user:reader", repo, Role::Reader);
    let a = Ag::bare();
    let run = |args: &[&str]| a.run_env("user:alice", args, &[("AG_TOKEN", &alice)]);
    run(&["init", "--namespace", repo]).ok();
    run(&["remote", "add", "origin", &format!("{}/{repo}", server.url), "--token-env", "AG_TOKEN"]).ok();
    a.write("file.txt", "body");
    run(&["snapshot", "-m", "target"]).ok();
    let proposal = run(&["--json", "push", "-t", "target"]).ok().json();
    let id = proposal["proposal"]["id"].as_str().unwrap();
    let head = proposal["proposal"]["head"].as_str().unwrap();
    a.write("notes.md", "Private comment\r\nsecond line\n");
    let args = [
        "--json",
        "remote",
        "comment-draft",
        "save",
        id,
        "--head",
        head,
        "--body-file",
        "notes.md",
        "--expected-version",
        "0",
    ];
    let first = run(&args).ok().json();
    assert_eq!(first["version"], 1);
    assert_eq!(first["head"], head);
    assert_eq!(first["body"], "Private comment\r\nsecond line\n");
    assert_eq!(run(&args).ok().json(), first);
    let other =
        a.run_env("user:bob", &["--json", "remote", "comment-draft", "get", id], &[("AG_TOKEN", &bob)]).ok().json();
    assert_eq!(other["present"], false);
    assert_eq!(run(&["--json", "remote", "comment-draft", "get", id]).ok().json(), first);
    assert!(
        !a.run_env("user:reader", &["remote", "comment-draft", "get", id], &[("AG_TOKEN", &reader)])
            .out
            .status
            .success()
    );
    assert!(run(&["--json", "remote", "discussion", "list", id]).ok().json()["threads"].as_array().unwrap().is_empty());

    assert!(
        !run(&["remote", "comment-draft", "save", id, "--head", head, "-m", "stale", "--expected-version", "0"])
            .out
            .status
            .success()
    );
    // A second role lets the same account also keep a distinct review-decision draft.
    server.user("user:alice", repo, Role::Reviewer);
    run(&[
        "remote",
        "review-draft",
        "save",
        id,
        "--head",
        head,
        "--decision",
        "approve",
        "-m",
        "Private review",
        "--expected-version",
        "0",
    ])
    .ok();
    assert_eq!(run(&["--json", "remote", "review-draft", "get", id]).ok().json()["body"], "Private review");
    assert_eq!(run(&["--json", "remote", "comment-draft", "get", id]).ok().json(), first);
    run(&["remote", "close", id]).ok();
    let closed = run(&[
        "--json",
        "remote",
        "comment-draft",
        "save",
        id,
        "--head",
        head,
        "-m",
        "Notes after closure",
        "--expected-version",
        "1",
    ])
    .ok()
    .json();
    assert_eq!(closed["version"], 2);
    assert_eq!(closed["body"], "Notes after closure");
    run(&["remote", "comment-draft", "clear", id]).code_is(2);
    assert!(!run(&["remote", "comment-draft", "clear", id, "--expected-version", "1"]).out.status.success());
    let cleared = run(&["--json", "remote", "comment-draft", "clear", id, "--expected-version", "2"]).ok().json();
    assert_eq!(cleared["present"], false);
    assert_eq!(cleared["version"], 3);
    assert!(!run(&args).out.status.success());
    let detail = run(&["--json", "remote", "proposal", id]).ok().json();
    assert!(detail["reviews"].as_array().unwrap().is_empty());
    assert_eq!(detail["proposal"]["head"], head);
}
