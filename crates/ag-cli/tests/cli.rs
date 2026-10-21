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
fn init_with_a_storage_file_checks_it_and_never_stores_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let ag = |args: &[&str]| Run {
        out: Command::new(env!("CARGO_BIN_EXE_ag"))
            .current_dir(dir.path())
            .env("AG_IDENTITY", "user:alice")
            .args(args)
            .output()
            .unwrap(),
    };
    // a password in a literal URL is refused, and the message does not repeat it
    fs::write(
        dir.path().join("bad.json"),
        r#"{"meta":{"kind":"postgres","url":"postgres://alice:hunter2@db.example/ag","schema":"ag_demo"}}"#,
    )
    .unwrap();
    let r = ag(&["init", "--storage", "bad.json"]).code_is(1);
    assert!(r.stderr().contains("password") && !r.stderr().contains("hunter2"), "{}", r.stderr());
    assert!(!dir.path().join(".ag").exists(), "nothing was created");

    // unknown keys and missing files are reported, not ignored
    fs::write(dir.path().join("typo.json"), r#"{"meta":{"kind":"postgres","schema":"s","url_envv":"X"}}"#).unwrap();
    let r = ag(&["init", "--storage", "typo.json"]).code_is(1);
    assert!(r.stderr().contains("url_envv"), "{}", r.stderr());
    ag(&["init", "--storage", "missing.json"]).code_is(1);

    // with a server: a PostgreSQL repository, usable through the CLI, whose backup is refused
    let Some(pg_url) = ag_testkit::require_here(
        "postgres",
        "AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)",
        ag_storage::testing::postgres_url(),
    ) else {
        return;
    };
    let schema = ag_storage::testing::TempSchema::new(&pg_url);
    fs::write(
        dir.path().join("pg.json"),
        format!(r#"{{"meta":{{"kind":"postgres","url_env":"AG_TEST_POSTGRES_URL","schema":"{}"}}}}"#, schema.name()),
    )
    .unwrap();
    ag(&["init", "--namespace", "acme/pg", "--storage", "pg.json"]).ok();
    let config = fs::read_to_string(dir.path().join(".ag/config.json")).unwrap();
    assert!(config.contains("AG_TEST_POSTGRES_URL") && !config.contains("postgres://"), "{config}");
    fs::write(dir.path().join("README.md"), "# pg\n").unwrap();
    ag(&["snapshot", "-m", "first"]).ok();
    ag(&["propose", "-t", "first"]).ok();
    let proposals = ag(&["--json", "proposal", "list"]).ok().json();
    let id = proposals[0]["id"].as_str().unwrap().to_string();
    ag(&["integrate", &id]).ok();
    ag(&["verify"]).ok();
    let r = ag(&["backup", "../backup"]).code_is(1);
    assert!(r.stderr().contains("pg_dump"), "{}", r.stderr());
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
fn renames_follow_edits_through_sync_and_integration() {
    let a = Ag::new();
    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    a.write("src/old.txt", &body);
    let base = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &base]).ok();

    // bob edits the last line in his own workspace
    let bob_dir = tempfile::tempdir().unwrap();
    let bob = bob_dir.path().join("bob");
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", bob.to_str().unwrap()]).ok();
    let edited = body.replace("line 10", "line 10 (bob)");
    fs::write(bob.join("src/old.txt"), &edited).unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob edit", "--workspace", "bob"]).ok();

    // alice moves the file and lands that
    fs::create_dir_all(a.root().join("moved")).unwrap();
    fs::rename(a.root().join("src/old.txt"), a.root().join("moved/new.txt")).unwrap();
    let pa = a.propose_as("user:alice", "move it");
    a.ag(&["integrate", &pa]).ok();

    // bob's sync merges cleanly: his edit lands in the moved file, the old path is gone
    let out = a.run_as("user:bob", &["sync", "--workspace", "bob"]).ok().stdout();
    assert!(out.to_lowercase().contains("merge"), "{out}");
    assert_eq!(fs::read_to_string(bob.join("moved/new.txt")).unwrap(), edited);
    assert!(!bob.join("src/old.txt").exists());
}

#[test]
fn a_content_conflict_after_a_rename_gets_markers_in_the_new_file() {
    let a = Ag::new();
    a.write("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
    let base = a.propose_as("user:alice", "base");
    a.ag(&["integrate", &base]).ok();
    let bob_dir = tempfile::tempdir().unwrap();
    let bob = bob_dir.path().join("bob");
    a.run_as("user:bob", &["workspace", "create", "bob", "--dir", bob.to_str().unwrap()]).ok();
    fs::write(bob.join("a.txt"), "1\n2\nBOB\n4\n5\n6\n7\n8\n").unwrap();
    a.run_as("user:bob", &["snapshot", "-m", "bob edit", "--workspace", "bob"]).ok();

    fs::rename(a.root().join("a.txt"), a.root().join("b.txt")).unwrap();
    a.write("b.txt", "1\n2\nALICE\n4\n5\n6\n7\n8\n");
    let pa = a.propose_as("user:alice", "rename and edit");
    a.ag(&["integrate", &pa]).ok();

    let r = a.run_as("user:bob", &["sync", "--workspace", "bob"]).code_is(4);
    assert!(r.stdout().contains("b.txt"), "{}", r.stdout());
    let marked = fs::read_to_string(bob.join("b.txt")).unwrap();
    assert!(marked.contains("<<<<<<< ours") && marked.contains("ALICE") && marked.contains("BOB"), "{marked}");
}

#[test]
fn blame_and_show_follow_a_rename() {
    let a = Ag::new();
    let body: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    a.write("old.txt", &body);
    let first = a.propose_as("user:alice", "create it");
    a.ag(&["integrate", &first]).ok();

    fs::remove_file(a.root().join("old.txt")).unwrap();
    a.write("new.txt", &body.replace("line 6", "line six"));
    let second = a.run_as("user:bob", &["snapshot", "-m", "rename and edit"]).ok();
    drop(second);
    let rev = a.run_as("user:bob", &["--json", "show", "@"]).ok().json();
    assert_eq!(rev["changes"][0]["op"], "R", "{rev}");
    assert_eq!(rev["changes"][0]["from"], "old.txt");
    assert_eq!(rev["changes"][0]["path"], "new.txt");
    let text = a.run_as("user:bob", &["show", "@"]).ok().stdout();
    assert!(text.contains("R old.txt -> new.txt"), "{text}");

    let out = a.run_as("user:bob", &["blame", "new.txt"]).ok().stdout();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "{out}");
    assert!(
        lines[0].contains("user:alice") && lines[0].contains("old.txt") && lines[0].ends_with("1) line 1"),
        "{out}"
    );
    assert!(lines[5].contains("user:bob") && lines[5].ends_with("6) line six"), "{out}");
    // a range and the JSON form
    let j = a.run_as("user:bob", &["--json", "blame", "-L", "2,3", "@:new.txt"]).ok().json();
    assert_eq!(j["lines"].as_array().unwrap().len(), 2);
    assert_eq!(j["lines"][0]["path"], "old.txt");
    a.run_as("user:bob", &["blame", "nope.txt"]).code_is(1);
}

#[test]
fn blame_and_show_follow_a_copy_only_when_asked() {
    let a = Ag::new();
    let body: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    a.write("orig.txt", &body);
    let first = a.propose_as("user:alice", "create it");
    a.ag(&["integrate", &first]).ok();

    a.write("sub/copy.txt", &format!("{body}and one more\n"));
    a.run_as("user:bob", &["snapshot", "-m", "copy it"]).ok();

    // without --copies the copy is a plain add, in the text and in JSON
    let plain = a.run_as("user:bob", &["--json", "show", "@"]).ok().json();
    assert_eq!(plain["changes"][0]["op"], "A", "{plain}");
    let blamed = a.run_as("user:bob", &["blame", "sub/copy.txt"]).ok().stdout();
    assert!(blamed.lines().all(|l| l.contains("user:bob")), "{blamed}");

    let rev = a.run_as("user:bob", &["--json", "show", "--copies", "@"]).ok().json();
    assert_eq!(rev["changes"][0]["op"], "C", "{rev}");
    assert_eq!(rev["changes"][0]["from"], "orig.txt");
    assert!(rev["changes"][0]["similarity"].as_u64().unwrap() >= 50);
    let text = a.run_as("user:bob", &["show", "--copies", "@"]).ok().stdout();
    assert!(text.contains("C orig.txt -> sub/copy.txt"), "{text}");

    let out = a.run_as("user:bob", &["blame", "--copies", "sub/copy.txt"]).ok().stdout();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 7, "{out}");
    assert!(lines[0].contains("user:alice") && lines[0].contains("orig.txt"), "{out}");
    assert!(lines[6].contains("user:bob") && lines[6].ends_with("7) and one more"), "{out}");
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
    // The check is any command that exits 0 on the candidate tree: `sh`/`grep` here, `cmd`/`findstr` on Windows.
    let check: &[&str] =
        if cfg!(windows) { &["cmd", "/c", "findstr 2 app.txt"] } else { &["sh", "-c", "grep -q 'version 2' app.txt"] };
    let mut argv = vec!["validate", "run", &p2, "--check", "tests", "--key", "ci", "--"];
    argv.extend_from_slice(check);
    let r = a.ag(&argv).ok();
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

#[test]
fn hostile_text_in_history_cannot_inject_terminal_escapes() {
    let a = Ag::new();
    a.write("f", "x");
    // an author controls the snapshot message and may put escape sequences in it
    a.ag(&["snapshot", "-m", "innocent\u{1b}[2J\u{1b}]0;owned\u{7}"]).ok();
    for args in [&["log"][..], &["show"], &["status"]] {
        let out = a.ag(args).ok().stdout();
        assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{args:?} leaked a control character: {out:?}");
    }
    assert!(a.ag(&["log"]).ok().stdout().contains("\\u{001b}"), "the escape is shown, not executed");
    // JSON mode keeps the data faithful (JSON escapes control characters itself)
    let j = a.ag(&["--json", "log"]).ok();
    assert!(!j.stdout().contains('\u{1b}'));
    assert!(j.json()[0]["message"].as_str().unwrap().contains('\u{1b}'));
}

/// `policy show|history`, `queue list|run` and `predict` took the stream as a positional argument while
/// `propose` and `policy set` took `--stream`. Both spellings now work everywhere (and mean the same); naming the
/// stream twice is a usage error.
#[test]
fn a_stream_can_be_named_positionally_or_with_the_option() {
    let a = Ag::new();
    a.write("f.txt", "x\n");
    let p = a.propose_as("user:alice", "first");
    a.ag(&["integrate", &p]).ok();
    a.ag(&["stream", "create", "dev", "--from", "main"]).ok();
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    a.ag(&["policy", "set", "policy.json", "--stream", "dev"]).ok();

    let positional = a.ag(&["--json", "policy", "show", "dev"]).ok().json();
    let option = a.ag(&["--json", "policy", "show", "--stream", "dev"]).ok().json();
    assert_eq!(positional, option);
    assert_eq!(positional["stream"], "dev");
    assert_eq!(positional["version"], 1, "the policy was stored on dev, not on main");
    assert_eq!(a.ag(&["--json", "policy", "show"]).ok().json()["version"], 0, "the default stream is unchanged");
    assert_eq!(
        a.ag(&["--json", "policy", "history", "dev"]).ok().json(),
        a.ag(&["--json", "policy", "history", "--stream", "dev"]).ok().json()
    );
    for cmd in [&["queue", "list"][..], &["queue", "run"], &["predict"]] {
        let positional = [&["--json"][..], cmd, &["dev"]].concat();
        let option = [&["--json"][..], cmd, &["--stream", "dev"]].concat();
        assert_eq!(a.ag(&positional).ok().json(), a.ag(&option).ok().json(), "{cmd:?}");
    }
    // Naming the stream twice is refused rather than guessed.
    let both = a.ag(&["policy", "show", "main", "--stream", "dev"]).code_is(2);
    assert!(both.stderr().contains("cannot be used with"), "{}", both.stderr());
}

/// `integrate --key` is an idempotency key, `validate run --key` names an issuer key. `--idempotency-key` is the
/// unambiguous spelling of the first and is interchangeable with it.
#[test]
fn integrate_takes_its_idempotency_key_as_key_or_idempotency_key() {
    let a = Ag::new();
    a.write("f.txt", "x\n");
    let p = a.propose_as("user:alice", "first");
    let first = a.ag(&["--json", "integrate", &p, "--idempotency-key", "k-1"]).ok().json();
    // Repeating the same request with the old spelling replays the recorded result instead of failing.
    let again = a.ag(&["--json", "integrate", &p, "--key", "k-1"]).ok().json();
    assert_eq!(first["operation_id"], again["operation_id"]);
    let help = a.ag(&["integrate", "--help"]).ok().stdout();
    assert!(help.contains("--idempotency-key"), "{help}");
}
