//! Golden tests for the `--json` output of the local commands.
//!
//! Every read-oriented command is run in a scratch repository and the *shape* of its JSON output (keys and value
//! types, no values) is compared with `tests/golden/json-shapes.json`. A change to a shape fails the test; if the
//! change is intended (a key was added: allowed within a schema version; a key was removed, renamed or retyped:
//! a breaking change that needs a new version), regenerate with
//!
//! ```text
//! AG_UPDATE_GOLDEN=1 cargo test -p agentgit-cli --test json_shapes
//! ```
//!
//! and `python3 scripts/gen_cli_docs.py` (which renders `docs/reference/cli-json.md` from the golden files).
//! A second test checks that no command of `ag --help` is missing from the golden files (or from the short list
//! of commands whose output is not JSON).
#![allow(missing_docs)]

mod common;

use std::fs;
use std::process::Command;

use common::Ag;
use common::golden::{Recorder, check_golden, golden_path};
use serde_json::Value;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn the_json_output_of_every_local_command_matches_the_documented_shape() {
    let mut r = Recorder::default();
    let a = Ag::bare();
    r.rec("init", &a.ag(&["--json", "init", "--namespace", "acme/demo"]).ok());

    // -- history and workspace ------------------------------------------------------------------
    a.write("README.md", "# demo\n");
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\n");
    r.rec("status", &a.ag(&["--json", "status"]).ok());
    r.rec("snapshot", &a.ag(&["--json", "snapshot", "-m", "first"]).ok());
    r.rec("snapshot", &a.ag(&["--json", "snapshot", "-m", "nothing changed"]).ok());
    let p1 = r.rec("propose", &a.ag(&["--json", "propose", "-t", "first"]).ok())["id"].as_str().unwrap().to_string();
    r.rec("proposal list", &a.ag(&["--json", "proposal", "list"]).ok());
    r.rec("proposal show", &a.ag(&["--json", "proposal", "show", &p1]).ok());
    let preview = r.rec("preview", &a.ag(&["--json", "preview", &p1]).ok());
    let tree = preview["candidate_tree"].as_str().unwrap().to_string();

    // -- policy: a blocked integration, a review, then success ----------------------------------------
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    r.rec("policy set", &a.ag(&["--json", "policy", "set", "policy.json"]).ok());
    r.rec("policy show", &a.ag(&["--json", "policy", "show"]).ok());
    r.rec("policy history", &a.ag(&["--json", "policy", "history"]).ok());
    r.rec("policy explain", &a.ag(&["--json", "policy", "explain"]).ok());
    r.rec("policy preset list", &a.ag(&["--json", "policy", "preset", "list"]).ok());
    r.rec("policy preset show", &a.ag(&["--json", "policy", "preset", "show", "team"]).ok());
    // Applying a preset replaces the policy this scenario relies on: do it in a repository of its own.
    let presets = Ag::new();
    r.rec("policy preset apply", &presets.ag(&["--json", "policy", "preset", "apply", "agents"]).ok());
    r.rec("init#preset", &Ag::bare().ag(&["--json", "init", "--namespace", "acme/preset", "--preset", "team"]).ok());
    let blocked = r.rec("integrate#blocked", &a.ag(&["--json", "integrate", &p1]).code_is(3));
    let op = blocked["operation_id"].as_str().unwrap().to_string();
    r.rec("op show", &a.ag(&["--json", "op", "show", &op]).ok());
    r.rec(
        "proposal review",
        &a.run_as("user:bob", &["--json", "proposal", "review", &p1, "--approve", "-m", "ok"]).ok(),
    );
    r.rec("op resume", &a.ag(&["--json", "op", "resume", &op]).ok());
    r.rec("integrate", &a.ag(&["--json", "integrate", &p1]).ok());
    r.rec("op list", &a.ag(&["--json", "op", "list"]).ok());
    r.rec("recover", &a.ag(&["--json", "recover"]).ok());

    // -- a second proposal through the queue, update and close -------------------------------------------
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\n");
    let p2 = a.propose_as("user:alice", "two");
    a.run_as("user:bob", &["proposal", "review", &p2, "--approve"]).ok();
    r.rec("queue add", &a.ag(&["--json", "queue", "add", &p2]).ok());
    r.rec("queue list", &a.ag(&["--json", "queue", "list"]).ok());
    r.rec("queue run", &a.ag(&["--json", "queue", "run"]).ok());
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\npub fn three() -> u32 { 3 }\n");
    let p3 = a.propose_as("user:alice", "three");
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\npub fn four() -> u32 { 4 }\n");
    a.ag(&["snapshot", "-m", "four"]).ok();
    r.rec("proposal update", &a.ag(&["--json", "proposal", "update", &p3]).ok());
    r.rec("predict", &a.ag(&["--json", "predict"]).ok());
    r.rec("proposal close", &a.ag(&["--json", "proposal", "close", &p3]).ok());

    r.rec("log", &a.ag(&["--json", "log"]).ok());
    r.rec("show", &a.ag(&["--json", "show", "main"]).ok());
    r.rec("diff", &a.ag(&["--json", "diff", "main~1", "main"]).ok());
    r.rec("blame", &a.ag(&["--json", "blame", "main:src/lib.rs"]).ok());
    r.rec("ls", &a.ag(&["--json", "ls", "main"]).ok());
    r.rec("portability", &a.ag(&["--json", "portability", "main", "--target", "windows"]).ok());
    let out = tempfile::tempdir().unwrap();
    r.rec("checkout", &a.ag(&["--json", "checkout", "main", out.path().join("co").to_str().unwrap()]).ok());

    // -- streams ----------------------------------------------------------------------------------------------
    r.rec("stream list", &a.ag(&["--json", "stream", "list"]).ok());
    r.rec("stream create", &a.ag(&["--json", "stream", "create", "dev", "--from", "main"]).ok());
    r.rec("stream show", &a.ag(&["--json", "stream", "show", "main"]).ok());
    r.rec("stream reflog", &a.ag(&["--json", "stream", "reflog", "main"]).ok());

    // -- validation -----------------------------------------------------------------------------------------------
    r.rec("validate keygen", &a.ag(&["--json", "validate", "keygen", "ci"]).ok());
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\npub fn five() -> u32 { 5 }\n");
    let p5 = a.propose_as("user:alice", "five");
    r.rec(
        "validate run",
        &a.run_as("service:ci", &["--json", "validate", "run", &p5, "--check", "tests", "--key", "ci", "--", "true"])
            .ok(),
    );
    let pv = a.ag(&["--json", "preview", &p5]).ok().json();
    r.rec("validate list", &a.ag(&["--json", "validate", "list", pv["candidate_tree"].as_str().unwrap()]).ok());
    let _ = tree;

    // -- agents, tasks, leases, provenance ---------------------------------------------------------------------------
    r.rec("agent register", &a.ag(&["--json", "agent", "register", "bot"]).ok());
    r.rec("agent list", &a.ag(&["--json", "agent", "list"]).ok());
    let t1 = r.rec("task create", &a.ag(&["--json", "task", "create", "Design", "--scope", "src/**"]).ok())["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t2 = a.ag(&["--json", "task", "create", "Build"]).ok().json()["id"].as_str().unwrap().to_string();
    let t3 = a.ag(&["--json", "task", "create", "Cancel me"]).ok().json()["id"].as_str().unwrap().to_string();
    r.rec("task depend", &a.ag(&["--json", "task", "depend", &t2, &t1]).ok());
    r.rec("task ready", &a.ag(&["--json", "task", "ready"]).ok());
    r.rec("task list", &a.ag(&["--json", "task", "list"]).ok());
    r.rec("task done", &a.ag(&["--json", "task", "done", &t1]).ok());
    r.rec("task cancel", &a.ag(&["--json", "task", "cancel", &t3]).ok());
    let start = r.rec("agent start", &a.ag(&["--json", "agent", "start", "bot", "--task", &t2]).ok());
    let (run_id, who) =
        (start["run_id"].as_str().unwrap().to_string(), start["identity"].as_str().unwrap().to_string());
    r.rec("agent heartbeat", &a.ag(&["--json", "agent", "heartbeat", &run_id]).ok());
    r.rec("agent runs", &a.ag(&["--json", "agent", "runs"]).ok());
    let l = r.rec("lease acquire", &a.run_as(&who, &["--json", "lease", "acquire", "docs/**", "--exclusive"]).ok());
    let lease = l["lease"]["id"].as_str().unwrap().to_string();
    r.rec("lease list", &a.ag(&["--json", "lease", "list"]).ok());
    // `status` again, now with an agent run on a task, a lease, and open proposals (shapes are merged).
    r.rec("status", &a.run_as(&who, &["--json", "status"]).ok());
    a.write("pending.txt", "p\n");
    let _ = a.propose_as(&who, "pending");
    r.rec("status", &a.run_as(&who, &["--json", "status"]).ok());
    r.rec("lease extend", &a.run_as(&who, &["--json", "lease", "extend", &lease, "--ttl", "2h"]).ok());
    r.rec("lease release", &a.run_as(&who, &["--json", "lease", "release", &lease]).ok());
    r.rec(
        "provenance attach",
        &a.ag(&["--json", "provenance", "attach", "@", "--agent", "bot", "--model", "m", "--check", "cargo test"]).ok(),
    );
    r.rec("provenance show", &a.ag(&["--json", "provenance", "show", "@"]).ok());
    r.rec("agent finish", &a.ag(&["--json", "agent", "finish", &run_id]).ok());
    r.rec("agent reap", &a.ag(&["--json", "agent", "reap"]).ok());
    r.rec("agent disable", &a.ag(&["--json", "agent", "disable", "bot"]).ok());
    r.rec("agent enable", &a.ag(&["--json", "agent", "enable", "bot"]).ok());

    // -- workspaces -----------------------------------------------------------------------------------------------------
    let ws = out.path().join("ws");
    r.rec("workspace create", &a.ag(&["--json", "workspace", "create", "side", "--dir", ws.to_str().unwrap()]).ok());
    r.rec("workspace list", &a.ag(&["--json", "workspace", "list"]).ok());
    // `sync` reports four different outcomes; record each one.
    let land = |file: &str, text: &str, title: &str| {
        a.write(file, text);
        let p = a.propose_as("user:alice", title);
        a.run_as("user:bob", &["proposal", "review", &p, "--approve"]).ok();
        a.ag(&["integrate", &p]).ok();
    };
    land("docs.md", "d\n", "docs");
    r.rec("sync", &a.ag(&["--json", "sync", "--workspace", "side"]).ok()); // fast-forward
    r.rec("sync", &a.ag(&["--json", "sync", "--workspace", "side"]).ok()); // up to date
    fs::write(ws.join("side.txt"), "s\n").unwrap();
    a.ag(&["snapshot", "-m", "side", "--workspace", "side"]).ok();
    land("docs.md", "d2\n", "docs 2");
    r.rec("sync", &a.ag(&["--json", "sync", "--workspace", "side"]).ok()); // merged
    fs::write(ws.join("docs.md"), "side's d\n").unwrap();
    a.ag(&["snapshot", "-m", "side docs", "--workspace", "side"]).ok();
    land("docs.md", "d3\n", "docs 3");
    r.rec("sync", &a.ag(&["--json", "sync", "--workspace", "side"]).code_is(4)); // conflicted
    r.rec("status", &a.ag(&["--json", "status", "--workspace", "side"]).ok()); // a workspace with a base and a pending merge
    r.rec("workspace remove", &a.ag(&["--json", "workspace", "remove", "side"]).ok());

    // -- audit, integrity, retention -----------------------------------------------------------------------------------
    r.rec("trace", &a.ag(&["--json", "trace"]).ok());
    r.rec("audit", &a.ag(&["--json", "audit"]).ok());
    r.rec("audit#list", &a.ag(&["--json", "audit", "--list"]).ok());
    r.rec("verify", &a.ag(&["--json", "verify"]).ok());
    r.rec("gc", &a.ag(&["--json", "gc", "--dry-run"]).ok());
    r.rec("reindex", &a.ag(&["--json", "reindex"]).ok());
    r.rec("backup", &a.ag(&["--json", "backup", out.path().join("bk").to_str().unwrap()]).ok());
    r.rec("pin", &a.ag(&["--json", "pin", "v1", "main"]).ok());
    r.rec("pins", &a.ag(&["--json", "pins"]).ok());
    r.rec("unpin", &a.ag(&["--json", "unpin", "v1"]).ok());

    // -- the Git bridge -----------------------------------------------------------------------------------------------------
    if have("git") {
        let g = out.path().join("gitrepo");
        fs::create_dir_all(&g).unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git").current_dir(&g).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        fs::write(g.join("a.txt"), "a\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        r.rec("git import", &a.ag(&["--json", "git", "import", g.to_str().unwrap(), "--stream", "imported"]).ok());
        r.rec(
            "git export",
            &a.ag(&["--json", "git", "export", "main", out.path().join("exported").to_str().unwrap()]).ok(),
        );
    }

    // -- setup and diagnosis ---------------------------------------------------------------------------------------------------
    r.rec("doctor", &a.ag(&["--json", "doctor"]).ok());
    r.rec("config list", &a.ag(&["--json", "config", "list"]).ok());
    r.rec("config get", &a.ag(&["--json", "config", "get", "identity"]).ok());
    r.rec("config set", &a.ag(&["--json", "config", "set", "color", "never"]).ok());
    r.rec("config path", &a.ag(&["--json", "config", "path"]).ok());
    r.rec("config unset", &a.ag(&["--json", "config", "unset", "color"]).ok());
    r.rec("version", &a.ag(&["--json", "version"]).ok());
    // A remote entry's local half (the server-backed commands are covered in json_shapes_remote.rs).
    r.rec(
        "remote add",
        &a.ag(&["--json", "remote", "add", "origin", "http://127.0.0.1:9/acme/demo", "--token-env", "AG_TOKEN"]).ok(),
    );
    r.rec("remote list", &a.ag(&["--json", "remote", "list"]).ok());
    r.rec("remote remove", &a.ag(&["--json", "remote", "remove", "origin"]).ok());

    check_golden("json-shapes.json", &r.shapes, if have("git") { &[] } else { &["git "] });
}

/// All leaf commands of `ag --help`, as "stream create".
fn leaf_commands() -> Vec<String> {
    fn help(path: &[String]) -> String {
        let o = Command::new(env!("CARGO_BIN_EXE_ag")).args(path).arg("--help").env("NO_COLOR", "1").output().unwrap();
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
    fn walk(path: Vec<String>, out: &mut Vec<String>) {
        let text = help(&path);
        let subs: Vec<String> = text
            .split("Commands:\n")
            .nth(1)
            .map(|rest| {
                rest.lines()
                    .take_while(|l| l.starts_with("  "))
                    .filter_map(|l| l.split_whitespace().next().map(str::to_string))
                    .filter(|n| n != "help")
                    .collect()
            })
            .unwrap_or_default();
        if subs.is_empty() {
            out.push(path.join(" "));
        }
        for s in subs {
            let mut p = path.clone();
            p.push(s);
            walk(p, out);
        }
    }
    let mut out = Vec::new();
    walk(Vec::new(), &mut out);
    out
}

#[test]
fn every_command_is_either_in_the_golden_files_or_documented_as_not_json() {
    let mut known: Vec<String> = Vec::new();
    for f in ["json-shapes.json", "json-shapes-remote.json"] {
        let doc: Value = serde_json::from_str(&fs::read_to_string(golden_path(f)).unwrap()).unwrap();
        known.extend(doc["commands"].as_object().unwrap().keys().map(|k| k.split('#').next().unwrap().to_string()));
        for r in doc["raw"].as_array().unwrap() {
            known.push(r.as_str().unwrap().to_string());
        }
    }
    let mut missing = Vec::new();
    for leaf in leaf_commands() {
        // `remote` subcommands list as "remote add"; the top-level `help` is a topic browser.
        if !known.contains(&leaf) {
            missing.push(leaf);
        }
    }
    assert!(
        missing.is_empty(),
        "these commands have no golden --json shape (add them to the scenario in json_shapes.rs / json_shapes_remote.rs, or to RAW if their output is not JSON): {missing:?}"
    );
}
