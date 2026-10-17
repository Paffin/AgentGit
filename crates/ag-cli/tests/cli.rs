//! End-to-end tests that drive the real `ag` binary.
#![allow(missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

struct Ag {
    dir: tempfile::TempDir,
}

struct Run {
    out: Output,
}

impl Run {
    fn code(&self) -> i32 {
        self.out.status.code().unwrap_or(-1)
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout()).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.stdout()))
    }
    #[track_caller]
    fn ok(self) -> Self {
        assert_eq!(self.code(), 0, "stdout: {}\nstderr: {}", self.stdout(), self.stderr());
        self
    }
    #[track_caller]
    fn code_is(self, c: i32) -> Self {
        assert_eq!(self.code(), c, "stdout: {}\nstderr: {}", self.stdout(), self.stderr());
        self
    }
}

impl Ag {
    fn new() -> Self {
        let a = Self { dir: tempfile::tempdir().unwrap() };
        a.run_as("user:alice", &["init", "--namespace", "acme/demo"]).ok();
        a
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn run_as(&self, who: &str, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_ag"))
            .current_dir(self.dir.path())
            .env("AG_IDENTITY", who)
            .env_remove("AG_KEY_DIR")
            .args(args)
            .output()
            .expect("run ag");
        Run { out }
    }

    fn ag(&self, args: &[&str]) -> Run {
        self.run_as("user:alice", args)
    }

    fn write(&self, rel: &str, data: &str) {
        let p = self.dir.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }

    /// snapshot + propose, returns the proposal id
    fn propose_as(&self, who: &str, msg: &str) -> String {
        self.run_as(who, &["snapshot", "-m", msg]).ok();
        self.run_as(who, &["--json", "propose", "-t", msg]).ok().json()["id"].as_str().unwrap().to_string()
    }
}

fn in_dir(p: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ag")).current_dir(p).env("AG_IDENTITY", "user:alice").args(args).output().unwrap()
}

#[test]
fn full_cycle_with_history_inspection() {
    let a = Ag::new();
    a.write("README.md", "# demo\n");
    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\n");
    let st = a.ag(&["status"]).ok().stdout();
    assert!(st.contains("added     README.md") && st.contains("added     src/lib.rs"), "{st}");

    let p1 = a.propose_as("user:alice", "initial import");
    let r = a.ag(&["integrate", &p1]).ok();
    assert!(r.stdout().contains("Integrated (fast-forward)"), "{}", r.stdout());
    assert!(a.ag(&["status"]).ok().stdout().contains("clean"));

    a.write("src/lib.rs", "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\n");
    let diff = a.ag(&["diff"]).ok().stdout();
    assert!(diff.contains("+pub fn two() -> u32 { 2 }"), "{diff}");
    let p2 = a.propose_as("user:alice", "add two");
    a.ag(&["integrate", &p2]).ok();

    let log = a.ag(&["log"]).ok().stdout();
    assert!(log.contains("add two") && log.contains("initial import"), "{log}");
    assert_eq!(a.ag(&["--json", "log", "main"]).ok().json().as_array().unwrap().len(), 2);
    let show = a.ag(&["show", "main"]).ok().stdout();
    assert!(show.contains("M src/lib.rs"), "{show}");
    assert!(a.ag(&["diff", "main~1", "main"]).ok().stdout().contains("+pub fn two"));
    assert_eq!(a.ag(&["cat", "main~1:src/lib.rs"]).ok().stdout(), "pub fn one() -> u32 { 1 }\n");
    assert!(a.ag(&["ls", "main"]).ok().stdout().contains("README.md"));
    assert!(a.ag(&["ls", "main", "src"]).ok().stdout().contains("lib.rs"));
    assert!(a.ag(&["log", "--path", "README.md"]).ok().stdout().contains("initial import"));
    assert!(!a.ag(&["log", "--path", "README.md"]).ok().stdout().contains("add two"));

    // checkout into a fresh directory reproduces the exact files
    let out = a.root().join("..").join(format!("{}-co", a.root().file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_dir_all(&out);
    a.ag(&["checkout", "main", out.to_str().unwrap()]).ok();
    assert_eq!(fs::read_to_string(out.join("src/lib.rs")).unwrap(), a.read("src/lib.rs"));
    let _ = fs::remove_dir_all(&out);

    let v = a.ag(&["verify"]).ok().stdout();
    assert!(v.contains("healthy"), "{v}");
    assert!(a.ag(&["audit"]).ok().stdout().contains("intact"));
}

#[test]
fn exit_codes_are_distinct_for_blocked_refused_usage_and_error() {
    let a = Ag::new();
    a.write("f.txt", "1\n2\n3\n");
    let base = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &base]).ok();

    // usage errors (clap) exit with 2; they can never be mistaken for "blocked"
    a.ag(&["no-such-command"]).code_is(2);
    a.ag(&["integrate"]).code_is(2);
    // runtime errors exit with 1
    a.ag(&["integrate", "prop_does_not_exist"]).code_is(1);

    // blocked (3): a missing approval is something the user can provide
    let pol = a.root().join("policy.json");
    fs::write(&pol, r#"{"approvals":{"min":1}}"#).unwrap();
    a.ag(&["policy", "set", pol.to_str().unwrap()]).ok();
    fs::remove_file(&pol).unwrap();
    a.write("f.txt", "1\nTWO-A\n3\n");
    let pa = a.propose_as("user:bob", "a");
    let r = a.ag(&["integrate", &pa]).code_is(3);
    assert!(r.stdout().contains("Blocked") && r.stdout().contains("ag proposal review"), "{}", r.stdout());
    a.run_as("user:carol", &["proposal", "review", &pa, "--approve", "-m", "ok"]).ok();
    a.ag(&["integrate", &pa]).ok();

    // refused (4): an agent may not integrate on its own
    a.write("g.txt", "g\n");
    let pg = a.propose_as("agent:bot", "g");
    a.run_as("agent:bot", &["integrate", &pg]).code_is(4);
}

#[test]
fn conflicts_exit_with_code_4_and_leave_the_stream_alone() {
    let a = Ag::new();
    a.write("f.txt", "1\n2\n3\n");
    let base = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &base]).ok();

    // bob works in his own workspace/directory, from the same base
    let bob_dir = tempfile::tempdir().unwrap();
    let bob = bob_dir.path().join("bob");
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", bob.to_str().unwrap()]).ok();
    assert!(a.ag(&["workspace", "list"]).ok().stdout().contains("bob"));

    // alice edits line 2 and lands it
    a.write("f.txt", "1\nALICE\n3\n");
    let pa = a.propose_as("user:alice", "alice edit");
    a.ag(&["integrate", &pa]).ok();
    let head_after_a = a.ag(&["--json", "stream", "show", "main"]).ok().json()["stream"]["state"]["head"].clone();

    // bob edits the same line and tries to integrate
    fs::write(bob.join("f.txt"), "1\nBOB\n3\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob edit", "--workspace", "bob"]).ok();
    let pb = a.run_as("user:bob", &["--json", "propose", "-t", "bob edit", "@bob"]).ok().json()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let pv = a.ag(&["preview", &pb]).code_is(4);
    assert!(pv.stdout().contains("conflict: f.txt"), "{}", pv.stdout());
    let r = a.ag(&["integrate", &pb]).code_is(4);
    assert!(r.stdout().contains("Conflict") && r.stdout().contains("f.txt"), "{}", r.stdout());
    assert_eq!(
        a.ag(&["--json", "stream", "show", "main"]).ok().json()["stream"]["state"]["head"],
        head_after_a,
        "stream untouched"
    );

    // bob syncs, sees markers, resolves, proposes the merge, and it lands
    a.run_as("user:bob", &["sync", "--workspace", "bob"]).code_is(4);
    let marked = fs::read_to_string(bob.join("f.txt")).unwrap();
    assert!(marked.contains("<<<<<<< ours") && marked.contains("ALICE") && marked.contains("BOB"), "{marked}");
    a.run_as("user:bob", &["snapshot", "-m", "too early", "--workspace", "bob"]).code_is(1);
    fs::write(bob.join("f.txt"), "1\nALICE+BOB\n3\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "resolve", "--workspace", "bob"]).ok();
    a.run_as("user:bob", &["proposal", "update", &pb, "@bob"]).ok();
    a.ag(&["integrate", &pb]).ok();
    assert_eq!(a.ag(&["cat", "main:f.txt"]).ok().stdout(), "1\nALICE+BOB\n3\n");
}

#[test]
fn validation_gate_with_signed_attestations() {
    let a = Ag::new();
    let key = a.ag(&["--json", "validate", "keygen", "ci"]).ok().json();
    let pubkey = key["public_key"].as_str().unwrap().to_string();
    let policy = format!(
        r#"{{"trusted_issuers":[{{"identity":"service:ci","public_key":"{pubkey}"}}],
             "required_validations":[{{"check":"tests","issuers":["service:ci"]}}]}}"#
    );
    let pf = a.root().join("policy.json");
    fs::write(&pf, policy).unwrap();
    a.ag(&["policy", "set", pf.to_str().unwrap()]).ok();
    fs::remove_file(&pf).unwrap();

    a.write("app.txt", "version 1\n");
    let p = a.propose_as("user:alice", "v1");
    let r = a.ag(&["integrate", &p]).code_is(3);
    assert!(
        r.stdout().contains("no trusted passing attestation") && r.stdout().contains("ag validate run"),
        "{}",
        r.stdout()
    );

    // a failing check is recorded and denies the exact tree
    a.ag(&["validate", "run", &p, "--check", "tests", "--key", "ci", "--", "false"]).code_is(4);
    let r = a.ag(&["integrate", &p]).code_is(4);
    assert!(r.stdout().contains("Denied") && r.stdout().contains("FAILED"), "{}", r.stdout());

    // a new proposal (new head, new tree) with a passing check lands
    a.write("app.txt", "version 2\n");
    let p2 = a.propose_as("user:alice", "v2");
    let r = a
        .ag(&[
            "validate",
            "run",
            &p2,
            "--check",
            "tests",
            "--key",
            "ci",
            "--",
            "sh",
            "-c",
            "grep -q 'version 2' app.txt",
        ])
        .ok();
    assert!(r.stdout().contains("PASSED"), "{}", r.stdout());
    a.ag(&["integrate", &p2]).ok();

    // the audit trail names who attested what
    let trace = a.ag(&["trace", "-n", "100"]).ok().stdout();
    assert!(trace.contains("attestation.submit") && trace.contains("service:ci"), "{trace}");
}

#[test]
fn agents_leases_and_provenance() {
    let a = Ag::new();
    a.write("src/a.rs", "fn a() {}\n");
    let p0 = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &p0]).ok();

    let pf = a.root().join("policy.json");
    fs::write(&pf, r#"{"require_provenance_for_agents":true}"#).unwrap();
    a.ag(&["policy", "set", pf.to_str().unwrap()]).ok();
    fs::remove_file(&pf).unwrap();

    a.ag(&["agent", "register", "coder"]).ok();
    let run = a.ag(&["--json", "agent", "start", "coder"]).ok().json();
    let who = run["identity"].as_str().unwrap().to_string();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert!(who.starts_with("agent:coder#run_"), "{who}");

    // the agent takes an exclusive lease and proposes
    let lease = a.run_as(&who, &["--json", "lease", "acquire", "src/**", "--exclusive", "--ttl", "10m"]).ok().json();
    let token = lease["lease"]["token"].as_u64().unwrap().to_string();
    a.write("src/a.rs", "fn a() { /* improved */ }\n");
    let prop = a.propose_as(&who, "improve a");

    // an agent may not integrate itself
    a.run_as(&who, &["integrate", &prop]).code_is(4);
    // a human integrator: blocked on provenance, then on approval, then lease token
    let r = a.ag(&["integrate", &prop]).code_is(3);
    assert!(r.stdout().contains("provenance"), "{}", r.stdout());
    a.run_as(
        &who,
        &["provenance", "attach", "--agent", "coder", "--run", &run_id, "--model", "demo-model", "--rationale", "tidy"],
    )
    .ok();
    let r = a.ag(&["integrate", &prop]).code_is(3);
    assert!(r.stdout().contains("approval"), "{}", r.stdout());
    a.run_as("user:carol", &["proposal", "review", &prop, "--approve"]).ok();
    let r = a.ag(&["integrate", &prop]).code_is(3);
    assert!(r.stdout().contains("exclusive lease"), "{}", r.stdout());
    a.ag(&["integrate", &prop, "--fencing-token", &token]).ok();

    let prov = a.ag(&["provenance", "show", "main"]).ok().stdout();
    assert!(prov.contains("VALID") && prov.contains("demo-model"), "{prov}");
    // finishing the run releases the lease
    a.run_as(&who, &["agent", "finish", &run_id]).ok();
    assert!(!a.ag(&["lease", "list"]).ok().stdout().contains("active"));
    // trace shows the agent's whole story
    let t = a.ag(&["trace", &run_id]).ok().stdout();
    assert!(t.contains("run.start") && t.contains("run.finish"), "{t}");
}

#[test]
fn tasks_respect_dependencies() {
    let a = Ag::new();
    a.ag(&["agent", "register", "bot"]).ok();
    let t1 = a.ag(&["--json", "task", "create", "design", "--scope", "api/**"]).ok().json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t2 = a.ag(&["--json", "task", "create", "build"]).ok().json()["id"].as_str().unwrap().to_string();
    a.ag(&["task", "depend", &t2, &t1]).ok();
    a.ag(&["task", "depend", &t1, &t2]).code_is(1); // cycle
    let ready = a.ag(&["--json", "task", "ready"]).ok().json();
    assert_eq!(ready.as_array().unwrap().len(), 1);
    a.ag(&["agent", "start", "bot", "--task", &t2]).code_is(1); // blocked
    a.ag(&["agent", "start", "bot", "--task", &t1]).ok();
}

#[test]
fn json_mode_is_valid_and_errors_go_to_stderr() {
    let a = Ag::new();
    a.write("x", "1");
    let p = a.propose_as("user:alice", "x");
    let r = a.ag(&["--json", "integrate", &p]).ok().json();
    assert_eq!(r["state"], "integrated");
    assert_eq!(r["outcome"]["status"], "integrated");
    let r = a.ag(&["show", "deadbeef00"]);
    assert_eq!(r.code(), 1);
    assert!(r.stderr().starts_with("error:"), "{}", r.stderr());
    assert!(r.stdout().is_empty());
    // outside a repository
    let tmp = tempfile::tempdir().unwrap();
    let out = in_dir(tmp.path(), &["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ag init"));
}

#[test]
fn gc_and_reindex_via_cli() {
    let a = Ag::new();
    a.write("x", "1");
    let p = a.propose_as("user:alice", "x");
    a.ag(&["integrate", &p]).ok();
    // an abandoned snapshot
    a.write("y", "garbage");
    a.ag(&["snapshot", "-m", "abandoned"]).ok();
    a.ag(&["pin", "keep", "main"]).ok();
    assert!(a.ag(&["pins"]).ok().stdout().contains("keep"));
    // grace 0 + dry run: the unproposed snapshot is still referenced by the workspace, so nothing is garbage
    let r = a.ag(&["gc", "--grace", "0s", "--dry-run"]).ok().stdout();
    assert!(r.contains("would delete 0"), "{r}");
    assert!(a.ag(&["reindex"]).ok().stdout().contains("Rebuilt"));
    assert!(a.ag(&["verify"]).ok().stdout().contains("healthy"));
    a.ag(&["unpin", "keep"]).ok();
    a.ag(&["unpin", "keep"]).code_is(1);
}

#[test]
fn predict_reports_overlapping_work() {
    let a = Ag::new();
    a.write("shared.txt", "base\n");
    let p0 = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &p0]).ok();
    a.ag(&["predict"]).ok();
    a.ag(&["lease", "acquire", "shared.txt", "--ttl", "5m"]).ok();
    let r = a.run_as("user:bob", &["lease", "acquire", "shared.txt", "--ttl", "5m"]).ok();
    assert!(r.stdout().contains("warning: overlaps"), "{}", r.stdout());
}
