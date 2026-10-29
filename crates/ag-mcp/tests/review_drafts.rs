//! Account drafts are private notes, not posted review decisions.
#![allow(missing_docs)]
mod common;
use common::*;
use serde_json::json;

#[test]
fn private_review_drafts_preserve_other_accounts_and_stale_edits() {
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
    let empty = bob.ok("review_draft_get", args.clone());
    assert_eq!(empty.structured["draft"]["present"], false);
    let save = json!({"proposal_id":id,"head":head,"decision":"comment","body":"Private notes","expected_version":0});
    let saved = bob.ok("review_draft_save", save.clone());
    assert_eq!(saved.structured["draft"]["version"], 1);
    assert_eq!(bob.ok("review_draft_save", save).structured["draft"]["version"], 1);
    assert_eq!(carol.ok("review_draft_get", args.clone()).structured["draft"]["present"], false);
    assert!(
        bob.call(
            "review_draft_save",
            json!({"proposal_id":id,"head":head,"decision":"comment","body":"stale replacement","expected_version":0})
        )
        .is_error
    );
    let clear = bob.ok("review_draft_clear", json!({"proposal_id":id,"expected_version":1}));
    assert_eq!(clear.structured["draft"]["present"], false);
    assert_eq!(clear.structured["draft"]["version"], 2);
    assert_eq!(bob.ok("review_draft_get", args.clone()).structured["draft"]["body"], "");
    assert!(
        bob.call(
            "review_draft_save",
            json!({"proposal_id":id,"head":head,"decision":"comment","body":"old device","expected_version":0})
        )
        .is_error
    );
    assert!(
        bob.call(
            "review_draft_save",
            json!({"proposal_id":id,"head":head,"decision":"comment","body":"missing guard"})
        )
        .is_error
    );
    let newer = bob.ok(
        "review_draft_save",
        json!({"proposal_id":id,"head":head,"decision":"comment","body":"Recovered note","expected_version":2}),
    );
    assert_eq!(newer.structured["draft"]["version"], 3);
    author.ok("proposal_close", json!({"proposal_id":id}));
    assert_eq!(bob.ok("review_draft_get", args).structured["draft"]["body"], "Recovered note");
    assert!(
        !bob.ok("review_draft_clear", json!({"proposal_id":id,"expected_version":3})).structured["draft"]["present"]
            .as_bool()
            .unwrap()
    );
}
