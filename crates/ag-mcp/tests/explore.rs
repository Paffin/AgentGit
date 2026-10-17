//! A real `ag-server` and a real `ag-mcp` over stdio: reading a repository.
#![allow(missing_docs)]

mod common;

use common::*;
use serde_json::json;

fn seeded() -> (TestServer, String) {
    let srv = TestServer::start();
    srv.repo("acme/web");
    srv.seed(
        "acme/web",
        "Initial import",
        &[
            ("README.md", "# Web\n\nA tiny web app.\n"),
            ("src/lib.rs", "pub fn greet() -> &'static str {\n    \"hello\"\n}\n"),
            ("src/auth/login.rs", "pub fn login() {}\n"),
        ],
    );
    srv.seed(
        "acme/web",
        "Friendlier greeting",
        &[("src/lib.rs", "pub fn greet() -> &'static str {\n    \"hello, world\"\n}\n")],
    );
    let reader = srv.credential("agent:reader", "acme/web", "reader", 3600, &[]);
    (srv, reader)
}

#[test]
fn read_only_exploration_over_stdio() {
    let (srv, reader) = seeded();
    let mut m = Mcp::start(&srv, &reader, &["--repo", "acme/web", "--allow", "read"]);

    // Only read tools are offered, and every one of them says so honestly.
    let tools = m.tools();
    assert!(tools.len() >= 14);
    for t in &tools {
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{}", t["name"]);
        assert_eq!(t["annotations"]["destructiveHint"], false, "{}", t["name"]);
        assert_eq!(t["annotations"]["openWorldHint"], false);
        assert!(t["outputSchema"].is_object() && t["inputSchema"]["additionalProperties"] == false);
    }
    let names = m.tool_names();
    for must in [
        "whoami",
        "repo_info",
        "log",
        "show",
        "blame",
        "tree",
        "read_file",
        "diff",
        "predict_conflicts",
        "proposal_status",
    ] {
        assert!(names.contains(&must.to_string()), "{must} missing from {names:?}");
    }
    for never in ["workspace_write_files", "propose", "task_claim", "lease_acquire", "review", "integrate"] {
        assert!(!names.contains(&never.to_string()), "{never} must not be offered with --allow read");
    }

    let who = m.ok("whoami", json!({}));
    assert_eq!(who.structured["identity"], "agent:reader");
    assert_eq!(who.structured["mcp"]["groups"], json!(["read"]));
    assert!(who.structured["expires_in_seconds"].as_i64().unwrap() > 3000);
    assert!(who.text.contains("Acting as agent:reader"), "{}", who.text);

    // The pinned repository is the default: no `repo` argument needed.
    let info = m.ok("repo_info", json!({}));
    assert_eq!(info.structured["repo"]["name"], "acme/web");
    assert_eq!(info.structured["streams"][0]["name"], "main");

    let log = m.ok("log", json!({"limit": 5}));
    let cs = log.structured["changesets"].as_array().unwrap();
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[0]["message"], "Friendlier greeting");
    assert_eq!(cs[1]["message"], "Initial import");
    let newest = cs[0]["id"].as_str().unwrap().to_string();
    let oldest = cs[1]["id"].as_str().unwrap().to_string();

    let log_path = m.ok("log", json!({"path": "README.md"}));
    assert_eq!(log_path.structured["changesets"].as_array().unwrap().len(), 1);

    let show = m.ok("show", json!({"rev": &newest, "include_diff": true}));
    assert_eq!(show.structured["changeset"]["ops"][0]["path"], "src/lib.rs");
    let patch = show.structured["diff"]["files"][0]["patch"].as_str().unwrap();
    assert!(patch.contains("-    \"hello\"") && patch.contains("+    \"hello, world\""), "{patch}");
    assert!(show.text.contains("kind=diff.patch"), "{}", show.text);

    let tree = m.ok("tree", json!({}));
    let entries: Vec<&str> =
        tree.structured["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(entries, ["README.md", "src"]);
    let rec = m.ok("tree", json!({"recursive": true, "path": "src"}));
    let paths: Vec<&str> =
        rec.structured["entries"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["src/auth/login.rs", "src/lib.rs"]);

    let file = m.ok("read_file", json!({"path": "src/lib.rs"}));
    assert_eq!(file.structured["content"], "pub fn greet() -> &'static str {\n    \"hello, world\"\n}\n");
    assert_eq!(file.structured["total_lines"], 3);
    assert_eq!(file.structured["truncated"], false);
    assert!(file.text.contains("kind=file.content"), "{}", file.text);
    let old = m.ok("read_file", json!({"path": "src/lib.rs", "rev": &oldest}));
    assert!(old.structured["content"].as_str().unwrap().contains("\"hello\""));
    let part = m.ok("read_file", json!({"path": "src/lib.rs", "start_line": 2, "end_line": 2}));
    assert_eq!(part.structured["content"], "    \"hello, world\"\n");

    let blame = m.ok("blame", json!({"path": "src/lib.rs"}));
    let lines = blame.structured["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1]["summary"], "Friendlier greeting");
    assert_eq!(lines[0]["summary"], "Initial import");
    assert!(blame.text.contains("kind=blame.listing") && blame.text.contains("hello, world"));

    let d = m.ok("diff", json!({"from": &oldest, "to": &newest}));
    assert_eq!(d.structured["files_changed"], 1);
    let d1 = m.ok("diff", json!({"to": &newest}));
    assert_eq!(d1.structured["files"][0]["path"], "src/lib.rs", "with only `to`, the first parent is the base");
    let droot = m.ok("diff", json!({"to": &oldest}));
    assert_eq!(droot.structured["files_changed"], 3, "a root changeset differs from nothing");

    assert!(m.ok("predict_conflicts", json!({})).structured["overlaps"].as_array().unwrap().is_empty());
    assert!(
        m.ok("proposal_list", json!({"state": "integrated"})).structured["proposals"].as_array().unwrap().len() >= 2
    );
    assert!(m.ok("task_list", json!({})).structured["tasks"].as_array().unwrap().is_empty());
    assert!(m.ok("lease_list", json!({})).structured["leases"].as_array().unwrap().is_empty());
    assert!(m.ok("queue_status", json!({})).structured["entries"].as_array().unwrap().is_empty());
    assert!(m.ok("workspace_list", json!({})).structured["workspaces"].is_array());
    let repos = m.ok("repo_list", json!({}));
    assert_eq!(repos.structured["repos"][0]["name"], "acme/web");

    // Errors are explained, not raw.
    let missing = m.refused("read_file", json!({"path": "nope.txt"}), "not_found");
    assert!(missing.text.contains("There is no file") && missing.text.contains("tree"), "{}", missing.text);
    let dir = m.refused("read_file", json!({"path": "src"}), "invalid_arguments");
    assert!(dir.text.contains("is a directory"));
    let bad = m.refused("log", json!({"limit": 5000}), "invalid_arguments");
    assert!(bad.text.contains("limit: must be <= 100"), "{}", bad.text);
    let unknown_arg = m.refused("log", json!({"nonsense": 1}), "invalid_arguments");
    assert!(unknown_arg.text.contains("nonsense: unknown argument"));
    let other_repo = m.refused("repo_info", json!({"repo": "evil/other"}), "repo_not_allowed");
    assert!(
        other_repo.text.contains("outside the allow-list") && other_repo.text.contains("Nothing was sent"),
        "{}",
        other_repo.text
    );
    let bad_repo = m.refused("repo_info", json!({"repo": "../../etc"}), "invalid_arguments");
    assert!(bad_repo.text.contains("owner/name"));

    // Resources: templates, a listing and file reads, with the same sanitising and bounds.
    let t = m.request("resources/templates/list", json!({}));
    assert_eq!(t["result"]["resourceTemplates"][0]["uriTemplate"], "agentgit://{owner}/{name}/{rev}/{path}");
    assert_eq!(t["result"]["ttlMs"], 300000);
    let l = m.request("resources/list", json!({}));
    let uris: Vec<&str> =
        l["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert!(uris.contains(&"agentgit://acme/web/main/"), "{uris:?}");
    assert!(uris.contains(&"agentgit://acme/web/main/src/"), "{uris:?}");
    assert!(uris.contains(&"agentgit://acme/web/main/README.md"), "{uris:?}");
    let r = m.request("resources/read", json!({"uri": "agentgit://acme/web/main/src/lib.rs"}));
    assert_eq!(r["result"]["contents"][0]["mimeType"], "text/x-rust");
    assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("hello, world"));
    assert_eq!(r["result"]["cacheScope"], "private");
    let r = m.request("resources/read", json!({"uri": "agentgit://acme/web/main/src/"}));
    let listing: serde_json::Value =
        serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(listing["entries"][0]["uri"], "agentgit://acme/web/main/src/auth/");
    let r = m.request("resources/read", json!({"uri": "agentgit://acme/web/main/nope.md"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let r = m.request("resources/read", json!({"uri": "agentgit://evil/other/main/x"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let r = m.request("resources/read", json!({"uri": "file:///etc/passwd"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");

    // Prompts that fit the allow-list.
    let p = m.request("prompts/list", json!({}));
    let names: Vec<&str> =
        p["result"]["prompts"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["explore_repo"]);
    let g =
        m.request("prompts/get", json!({"name": "explore_repo", "arguments": {"question": "Where is login handled?"}}));
    assert!(g["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("Where is login handled?"));
}

#[test]
fn a_reader_credential_cannot_write_even_if_the_mcp_server_would_let_it() {
    let (srv, reader) = seeded();
    // The operator switched the write tools on, but the credential is only a reader: the server decides.
    let mut m = Mcp::start(&srv, &reader, &["--repo", "acme/web"]);
    let r = m.refused("workspace_create", json!({"name": "mine"}), "permission_denied");
    assert!(r.text.contains("credential's scope or this identity's role"), "{}", r.text);
    assert!(r.text.contains("Do not retry") && r.text.contains("kind=server.error"), "{}", r.text);
    assert_eq!(r.raw["_meta"]["agentgit/error"]["server"]["status"], 403);
}
