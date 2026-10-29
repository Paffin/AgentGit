//! Account drafts are private notes, not posted review decisions.
#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::{Value, json};

#[test]
fn private_review_drafts_preserve_other_accounts_and_stale_edits() {
    exercise_drafts("review");
}

#[test]
fn private_comment_drafts_work_after_closure_without_posting() {
    exercise_drafts("comment");
}

fn exercise_drafts(kind: &str) {
    let get = format!("{kind}_draft_get");
    let save_tool = format!("{kind}_draft_save");
    let clear_tool = format!("{kind}_draft_clear");
    let draft_args = |mut value: Value| {
        if kind == "comment" {
            value.as_object_mut().unwrap().remove("decision");
        }
        value
    };
    let srv = TestServer::start();
    let repo = "acme/drafts";
    srv.repo(repo);
    srv.seed(repo, "Initial", &[("file.txt", "old\n")]);
    let mut author = Mcp::start(&srv, ADMIN, &["--repo", repo]);
    author.ok("workspace_create", json!({"name":"change"}));
    author.ok(
        "workspace_write_files",
        json!({"workspace":"change","message":"change","changes":[{"path":"file.txt","op":"put","content":"new\n"}]}),
    );
    let proposed = author.ok("propose", json!({"workspace":"change","title":"Review"}));
    let id = proposed.structured["proposal"]["id"].as_str().unwrap();
    let head = proposed.structured["proposal"]["head"].as_str().unwrap();
    let bob_token = srv.credential("user:bob", repo, "reviewer", 3600, &[]);
    let carol_token = srv.credential("user:carol", repo, "reviewer", 3600, &[]);
    let mut bob = Mcp::start(&srv, &bob_token, &["--repo", repo, "--allow", "read,review"]);
    let mut carol = Mcp::start(&srv, &carol_token, &["--repo", repo, "--allow", "read,review"]);
    let args = json!({"proposal_id":id});
    let empty = bob.ok(&get, args.clone());
    assert_eq!(empty.structured["draft"]["present"], false);
    let save = draft_args(
        json!({"proposal_id":id,"head":head,"decision":"comment","body":"Private notes","expected_version":0}),
    );
    let saved = bob.ok(&save_tool, save.clone());
    assert_eq!(saved.structured["draft"]["version"], 1);
    assert_eq!(bob.ok(&save_tool, save).structured["draft"]["version"], 1);
    assert_eq!(carol.ok(&get, args.clone()).structured["draft"]["present"], false);
    if kind == "comment" {
        // Character limits alone must not permit an oversized UTF-8 body.
        bob.refused(
            &save_tool,
            json!({"proposal_id":id,"head":head,"body":"é".repeat(8193),"expected_version":1}),
            "invalid_arguments",
        );
        assert_eq!(bob.ok(&get, args.clone()).structured["draft"]["version"], 1);
        bob.ok("review_draft_save", json!({"proposal_id":id,"head":head,"decision":"comment","body":"Separate review note","expected_version":0}));
        assert_eq!(bob.ok(&get, args.clone()).structured["draft"]["body"], "Private notes");
    }
    assert!(
        bob.call(
            &save_tool,
            draft_args(json!({"proposal_id":id,"head":head,"decision":"comment","body":"stale replacement","expected_version":0}))
        )
        .is_error
    );
    let clear = bob.ok(&clear_tool, json!({"proposal_id":id,"expected_version":1}));
    assert_eq!(clear.structured["draft"]["present"], false);
    assert_eq!(clear.structured["draft"]["version"], 2);
    assert_eq!(bob.ok(&get, args.clone()).structured["draft"]["body"], "");
    assert!(
        bob.call(
            &save_tool,
            draft_args(
                json!({"proposal_id":id,"head":head,"decision":"comment","body":"old device","expected_version":0})
            )
        )
        .is_error
    );
    assert!(
        bob.call(
            &save_tool,
            draft_args(json!({"proposal_id":id,"head":head,"decision":"comment","body":"missing guard"}))
        )
        .is_error
    );
    let newer = bob.ok(
        &save_tool,
        draft_args(
            json!({"proposal_id":id,"head":head,"decision":"comment","body":"Recovered note","expected_version":2}),
        ),
    );
    assert_eq!(newer.structured["draft"]["version"], 3);
    author.ok("proposal_close", json!({"proposal_id":id}));
    if kind == "comment" {
        let closed =
            bob.ok(&save_tool, json!({"proposal_id":id,"head":head,"body":"Closed note","expected_version":3}));
        assert_eq!(closed.structured["draft"]["version"], 4);
        bob.ok(&clear_tool, json!({"proposal_id":id,"expected_version":4}));
        assert_eq!(bob.ok("review_draft_get", args).structured["draft"]["body"], "Separate review note");
        let (status, public) = srv.admin().get(&format!("/v1/repos/{repo}/proposals/{id}"));
        assert_eq!(status, 200);
        assert_eq!(public["reviews"].as_array().unwrap().len(), 0);
        return;
    }
    assert_eq!(bob.ok(&get, args).structured["draft"]["body"], "Recovered note");
    assert!(
        !bob.ok(&clear_tool, json!({"proposal_id":id,"expected_version":3})).structured["draft"]["present"]
            .as_bool()
            .unwrap()
    );
}
