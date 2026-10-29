//! The `ag` binary against a real in-process `ag-server`: clone, push, review, integrate, pull.
#![allow(missing_docs)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use ag_server::app::App;
use ag_server::auth::mint_token;
use ag_server::authz::Role;
use ag_server::config::ServerConfig;
use ag_server::control::CredentialRow;
use ag_storage::SystemClock;
use serde_json::Value;

struct Server {
    url: String,
    app: App,
    _dir: tempfile::TempDir,
    _thread: std::thread::JoinHandle<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    fn start() -> Self {
        Self::start_with(|_| {})
    }

    fn start_with(tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.limits.requests_per_second = 0;
        tweak(&mut cfg);
        let app =
            App::with_options(cfg, Arc::new(SystemClock), Some("bootstrap-token-0123456789abcdef".into())).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = app.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                ag_server::server::serve(a, listener, async {
                    let _ = stop_rx.await;
                })
                .await
                .unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        Self { url: format!("http://{addr}"), app, _dir: dir, _thread: thread, stop: Some(stop_tx) }
    }

    fn user(&self, subject: &str, repo: &str, role: Role) -> String {
        self.app.control.grant_role(subject, repo, role, "system:test").unwrap();
        self.app.invalidate_bindings();
        let m = mint_token().unwrap();
        let now = self.app.clock.now().as_millis();
        self.app
            .control
            .insert_credential(&CredentialRow {
                id: m.credential_id,
                secret_hash: m.secret_hash,
                subject: subject.into(),
                kind: "user".into(),
                description: String::new(),
                scope: Default::default(),
                created_at_ms: now,
                expires_at_ms: now + 3_600_000,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "system:test".into(),
            })
            .unwrap();
        m.token
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

fn ag(dir: &Path, who: &str, token: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(dir)
        .env("AG_IDENTITY", who)
        .env("AG_TOKEN", token)
        .env_remove("AG_KEY_DIR")
        .args(args)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[track_caller]
fn ok(o: Output) -> String {
    assert!(o.status.success(), "{}", text(&o));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn proposal_id(out: &str) -> String {
    out.split_whitespace()
        .find(|w| w.starts_with("prop_"))
        .expect("a proposal id in the output")
        .trim_end_matches(['.', ','])
        .to_string()
}

#[test]
fn clone_push_review_integrate_and_pull() {
    let srv = Server::start();
    srv.app
        .repos
        .create("acme/demo", "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
        .unwrap();
    let alice = srv.user("user:alice", "acme/demo", Role::Writer);
    let bob = srv.user("user:bob", "acme/demo", Role::Reviewer);
    let carol = srv.user("user:carol", "acme/demo", Role::Integrator);
    let bob_writer = srv.user("user:dave", "acme/demo", Role::Writer);
    let url = format!("{}/acme/demo", srv.url);

    // Policy: one approval needed (set through the CLI by a bootstrap admin).
    let admin_dir = tempfile::tempdir().unwrap();
    ok(ag(admin_dir.path(), "user:root", "bootstrap-token-0123456789abcdef", &["init", "--namespace", "acme/demo"]));
    ok(ag(
        admin_dir.path(),
        "user:root",
        "bootstrap-token-0123456789abcdef",
        &["remote", "add", "origin", &url, "--token-env", "AG_TOKEN"],
    ));
    fs::write(admin_dir.path().join("policy.json"), r#"{"approvals": {"min": 1}}"#).unwrap();
    ok(ag(
        admin_dir.path(),
        "user:root",
        "bootstrap-token-0123456789abcdef",
        &["remote", "policy", "main", "--set", "policy.json"],
    ));

    // Alice starts from a local repository, pushes it as a proposal.
    let a = tempfile::tempdir().unwrap();
    ok(ag(a.path(), "user:alice", &alice, &["init", "--namespace", "acme/demo"]));
    ok(ag(a.path(), "user:alice", &alice, &["remote", "add", "origin", &url, "--token-env", "AG_TOKEN"]));
    fs::create_dir_all(a.path().join("src")).unwrap();
    fs::write(a.path().join("README.md"), "# demo\n").unwrap();
    fs::write(a.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\nfn three() {}\n").unwrap();
    ok(ag(a.path(), "user:alice", &alice, &["snapshot", "-m", "initial"]));
    let out = ok(ag(a.path(), "user:alice", &alice, &["push", "-t", "Initial import"]));
    let prop = proposal_id(&out);

    // Not approved yet: carol is blocked (exit code 3), and the report says why.
    let o = ag(a.path(), "user:carol", &carol, &["remote", "integrate", &prop]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("approvals"), "{}", text(&o));
    // Prepare reports the same without side effects, with a distinct exit code.
    let o = ag(a.path(), "user:carol", &carol, &["remote", "prepare", &prop]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    let explanation = ag(a.path(), "user:carol", &carol, &["--json", "remote", "explain", &prop]);
    assert_eq!(explanation.status.code(), Some(3), "{}", text(&explanation));
    let explained: Value = serde_json::from_slice(&explanation.stdout).unwrap();
    assert_eq!(explained["standing"], "blocked");
    assert_eq!(explained["explanations"][0]["code"], "insufficient_approvals");
    assert!(explained["next"][0].as_str().unwrap().starts_with("ag remote review"));
    // Line discussions use the same durable state as the browser.
    let detail: Value =
        serde_json::from_str(&ok(ag(a.path(), "user:alice", &alice, &["--json", "remote", "proposal", &prop])))
            .unwrap();
    let inspected = detail["proposal"]["head"].as_str().unwrap();
    let thread: Value = serde_json::from_str(&ok(ag(
        a.path(),
        "user:alice",
        &alice,
        &[
            "--json",
            "remote",
            "discussion",
            "start",
            &prop,
            "--head",
            inspected,
            "--path",
            "src/lib.rs",
            "--line",
            "1",
            "-m",
            "Why this function?",
        ],
    )))
    .unwrap();
    let thread_id = thread["id"].as_str().unwrap();
    assert_eq!(thread["anchor"]["text"], "fn one() {}");
    let listed: Value = serde_json::from_str(&ok(ag(
        a.path(),
        "user:alice",
        &alice,
        &["--json", "remote", "discussion", "list", &prop],
    )))
    .unwrap();
    assert_eq!(listed["threads"].as_array().unwrap().len(), 1);
    ok(ag(
        a.path(),
        "user:bob",
        &bob,
        &["remote", "discussion", "reply", &prop, thread_id, "-m", "It is the public entry point."],
    ));
    let replies: Value = serde_json::from_str(&ok(ag(
        a.path(),
        "user:alice",
        &alice,
        &["--json", "remote", "discussion", "replies", &prop, thread_id],
    )))
    .unwrap();
    assert_eq!(replies["comments"].as_array().unwrap().len(), 1);
    assert!(
        !ag(
            a.path(),
            "user:alice",
            &alice,
            &["remote", "discussion", "resolve", &prop, thread_id, "--expected-version", "1"]
        )
        .status
        .success()
    );
    let resolved: Value = serde_json::from_str(&ok(ag(
        a.path(),
        "user:alice",
        &alice,
        &["--json", "remote", "discussion", "resolve", &prop, thread_id, "--expected-version", "2"],
    )))
    .unwrap();
    assert_eq!(resolved["resolved"], true);
    // Discussion does not need or grant approval rights.
    ok(ag(
        a.path(),
        "user:alice",
        &alice,
        &["remote", "review", &prop, "--comment", "-m", "The explicit branch is intentional."],
    ));
    assert!(!ag(a.path(), "user:alice", &alice, &["remote", "review", &prop, "--approve"]).status.success());
    ok(ag(a.path(), "user:bob", &bob, &["remote", "review", &prop, "--approve", "-m", "lgtm"]));
    ok(ag(a.path(), "user:bob", &bob, &["remote", "review", &prop, "--comment", "-m", "Approval still stands."]));
    let ready = ok(ag(a.path(), "user:carol", &carol, &["--json", "remote", "explain", &prop]));
    assert_eq!(serde_json::from_str::<Value>(&ready).unwrap()["standing"], "ready");
    let out = ok(ag(a.path(), "user:carol", &carol, &["remote", "integrate", &prop]));
    assert!(out.contains("Integrated"), "{out}");
    let closed = ag(a.path(), "user:carol", &carol, &["remote", "explain", &prop]);
    assert_eq!(closed.status.code(), Some(4), "{}", text(&closed));
    assert!(text(&closed).contains("already landed"), "{}", text(&closed));

    ok(ag(a.path(), "user:alice", &alice, &["remote", "review", &prop, "--comment", "-m", "Confirmed after landing."]));

    // Dave clones: all files arrive, byte for byte.
    let d = tempfile::tempdir().unwrap();
    let dave_dir = d.path().join("demo");
    let o = ag(d.path(), "user:dave", &bob_writer, &["clone", &url, dave_dir.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(fs::read_to_string(dave_dir.join("src/lib.rs")).unwrap(), "fn one() {}\nfn two() {}\nfn three() {}\n");
    // Alice and Dave now edit different parts of the same file.
    fs::write(dave_dir.join("src/lib.rs"), "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\n").unwrap();
    ok(ag(&dave_dir, "user:dave", &bob_writer, &["snapshot", "-m", "add four"]));
    let dave_prop = proposal_id(&ok(ag(&dave_dir, "user:dave", &bob_writer, &["push", "-t", "Add four"])));

    // Alice pulls (nothing new yet beyond her own push), edits the first line, and pushes.
    ok(ag(a.path(), "user:alice", &alice, &["pull"]));
    fs::write(a.path().join("src/lib.rs"), "fn uno() {}\nfn two() {}\nfn three() {}\n").unwrap();
    ok(ag(a.path(), "user:alice", &alice, &["snapshot", "-m", "rename one"]));
    let alice_prop = proposal_id(&ok(ag(a.path(), "user:alice", &alice, &["push", "-t", "Rename one"])));

    for p in [&dave_prop, &alice_prop] {
        ok(ag(a.path(), "user:bob", &bob, &["remote", "review", p, "--approve"]));
    }
    ok(ag(a.path(), "user:carol", &carol, &["remote", "integrate", &dave_prop]));
    // The second integration is a real merge of two independent edits.
    let out = ok(ag(a.path(), "user:carol", &carol, &["remote", "integrate", &alice_prop]));
    assert!(out.contains("Integrated (merge)"), "{out}");

    // Pulling brings everything into both working directories.
    ok(ag(a.path(), "user:alice", &alice, &["pull"]));
    ok(ag(&dave_dir, "user:dave", &bob_writer, &["pull"]));
    let expected = "fn uno() {}\nfn two() {}\nfn three() {}\nfn four() {}\n";
    assert_eq!(fs::read_to_string(a.path().join("src/lib.rs")).unwrap(), expected);
    assert_eq!(fs::read_to_string(dave_dir.join("src/lib.rs")).unwrap(), expected);

    // The server tells the same story.
    let cat = ok(ag(a.path(), "user:alice", &alice, &["remote", "cat", "main:src/lib.rs"]));
    assert_eq!(cat, expected);
    let blame = ok(ag(a.path(), "user:alice", &alice, &["remote", "blame", "src/lib.rs"]));
    let rows: Vec<&str> = blame.lines().collect();
    assert_eq!(rows.len(), 4, "{blame}");
    assert!(rows[0].contains("user:alice") && rows[0].ends_with("1) fn uno() {}"), "{blame}");
    assert!(rows[3].contains("user:dave") && rows[3].ends_with("4) fn four() {}"), "{blame}");
    let log = ok(ag(a.path(), "user:alice", &alice, &["remote", "log"]));
    assert!(log.contains("Initial") || log.contains("initial"), "{log}");
    let who: Value =
        serde_json::from_str(&ok(ag(a.path(), "user:alice", &alice, &["--json", "remote", "whoami"]))).unwrap();
    assert_eq!(who["identity"], "user:alice");

    // A wrong credential is a clear error, not a hang.
    let o = ag(a.path(), "user:alice", "agt_0000000000000000_wrong", &["remote", "whoami"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("unauthenticated") || text(&o).contains("credentials"), "{}", text(&o));
}

#[test]
fn tokens_can_be_stored_with_restricted_permissions() {
    let srv = Server::start();
    srv.app
        .repos
        .create("acme/demo", "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
        .unwrap();
    let alice = srv.user("user:alice", "acme/demo", Role::Writer);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("clone");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(dir.path())
        .env("AG_IDENTITY", "user:alice")
        .args(["clone", &format!("{}/acme/demo", srv.url), target.to_str().unwrap(), "--token-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(alice.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let secret = target.join(".ag/credentials/origin");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);
    }
    #[cfg(windows)]
    {
        // No mode bits: the ACL must not inherit anything from the parent (that is where "Users" and
        // "Authenticated Users" come from) and must not name a broad group.
        let acl = Command::new("icacls").arg(&secret).output().unwrap();
        let acl = String::from_utf8_lossy(&acl.stdout).to_string();
        assert!(!acl.contains("(I)"), "no inherited entries on a stored credential:\n{acl}");
        for broad in ["Everyone", "BUILTIN\\Users", "Authenticated Users", "INTERACTIVE"] {
            assert!(!acl.contains(broad), "{broad} must not have access to a stored credential:\n{acl}");
        }
        assert!(acl.contains("(F)"), "the owner keeps full control:\n{acl}");
    }
    // Later commands find the stored credential without any environment variable.
    let o = Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(&target)
        .env("AG_IDENTITY", "user:alice")
        .env_remove("AG_TOKEN")
        .args(["remote", "whoami"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    // And a world-readable credential file is refused (Unix only: Windows cannot read an ACL back, see the docs).
    #[cfg(unix)]
    {
        fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_ag"))
            .current_dir(&target)
            .env("AG_IDENTITY", "user:alice")
            .env_remove("AG_TOKEN")
            .args(["remote", "whoami"])
            .output()
            .unwrap();
        assert!(!o.status.success());
        assert!(text(&o).contains("readable by other users"), "{}", text(&o));
    }
}

#[test]
fn an_operator_reaps_the_stale_runs_of_a_server_from_the_command_line() {
    let srv = Server::start_with(|cfg| cfg.agents.min_stale_after_secs = 1);
    let repo = "acme/demo";
    srv.app
        .repos
        .create(repo, "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
        .unwrap();
    let key = ag_crypto::KeyPair::generate().unwrap();
    let meta = srv.app.repos.handle(repo).unwrap();
    ag_agent::registry::register_agent_with_key(meta.meta(), "bot", &key.public(), "agent", "system:test").unwrap();
    let run = ag_agent::registry::start_run(meta.meta(), "bot", None).unwrap().run_id;
    std::thread::sleep(std::time::Duration::from_millis(1300));

    let boss = srv.user("user:boss", repo, Role::Delegator);
    let writer = srv.user("user:alice", repo, Role::Writer);
    let url = format!("{}/{repo}", srv.url);
    let dir = tempfile::tempdir().unwrap();
    ok(ag(dir.path(), "user:boss", &boss, &["init", "--namespace", repo]));
    ok(ag(dir.path(), "user:boss", &boss, &["remote", "add", "origin", &url, "--token-env", "AG_TOKEN"]));

    // Only an operator reaps: a plain writer is refused, naming the permission, and nothing changes.
    let o = ag(dir.path(), "user:alice", &writer, &["agent", "reap", "--remote"]);
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("agent.operate"), "{}", text(&o));
    // A threshold below the server's minimum is refused, not applied.
    let o = ag(dir.path(), "user:boss", &boss, &["agent", "reap", "--remote", "--timeout", "100ms"]);
    assert!(!o.status.success() && text(&o).contains("min_stale_after_secs"), "{}", text(&o));

    // A dry run says what it would do and changes nothing.
    let dry: Value = serde_json::from_str(&ok(ag(
        dir.path(),
        "user:boss",
        &boss,
        &["--json", "agent", "reap", "--remote", "--dry-run", "--timeout", "1s"],
    )))
    .unwrap();
    assert_eq!(dry["dry_run"], true, "{dry}");
    assert_eq!(dry["runs"][0]["run_id"], run.as_str(), "{dry}");
    assert_eq!(meta.meta().read(|tx| tx.run(&run)).unwrap().unwrap().status, "running");

    let out = ok(ag(dir.path(), "user:boss", &boss, &["agent", "reap", "--remote", "origin", "--timeout", "1s"]));
    assert!(out.contains("Abandoned 1 run(s)") && out.contains(&run), "{out}");
    assert_eq!(meta.meta().read(|tx| tx.run(&run)).unwrap().unwrap().status, "abandoned");
    // It is on the record, under the operator's identity.
    let audit = meta.meta().read(|tx| tx.audit_after(0, 1000)).unwrap();
    assert!(audit.iter().any(|e| e.action == "run.abandon" && e.actor == "user:boss"), "{audit:?}");
    // Nothing left to reap the second time.
    let again = ok(ag(dir.path(), "user:boss", &boss, &["agent", "reap", "--remote", "--timeout", "1s"]));
    assert!(again.contains("Abandoned 0 run(s)"), "{again}");
}

#[test]
fn remote_task_edit_uses_the_server_and_enforces_its_permissions() {
    let srv = Server::start();
    let repo = "acme/task-edit";
    srv.app
        .repos
        .create(repo, "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
        .unwrap();
    let alice = srv.user("user:alice", repo, Role::Writer);
    let bob = srv.user("user:bob", repo, Role::Writer);
    let handle = srv.app.repos.handle(repo).unwrap();
    let task =
        ag_agent::tasks::create_task(handle.meta(), "Initial", "Before", &["src/**".into()], "user:alice").unwrap();
    let dir = tempfile::tempdir().unwrap();
    ok(ag(dir.path(), "user:alice", &alice, &["init", "--namespace", repo]));
    ok(ag(
        dir.path(),
        "user:alice",
        &alice,
        &["remote", "add", "origin", &format!("{}/{repo}", srv.url), "--token-env", "AG_TOKEN"],
    ));
    let updated: Value = serde_json::from_str(&ok(ag(
        dir.path(),
        "user:alice",
        &alice,
        &["--json", "task", "edit", &task.id, "--remote", "--title", "Remote λ", "--clear-scope"],
    )))
    .unwrap();
    assert_eq!(updated["title"], "Remote λ");
    assert_eq!(updated["scope"], serde_json::json!([]));
    let attempts: Value = serde_json::from_str(&ok(ag(
        dir.path(),
        "user:alice",
        &alice,
        &["--json", "task", "attempts", &task.id, "--remote", "--max", "2", "--expected", "0"],
    )))
    .unwrap();
    assert_eq!(attempts["max_attempts"], 2);
    assert_eq!(attempts["attempts_started"], 0);
    assert!(
        !ag(
            dir.path(),
            "user:alice",
            &alice,
            &["task", "attempts", &task.id, "--remote", "--max", "3", "--expected", "0"]
        )
        .status
        .success()
    );
    let saved = handle.meta().read(|tx| tx.task(&task.id)).unwrap().unwrap();
    assert_eq!(saved.title, "Remote λ");
    assert_eq!(saved.description, "Before");
    let refused = ag(dir.path(), "user:bob", &bob, &["task", "edit", &task.id, "--remote", "--title", "Stolen"]);
    assert!(!refused.status.success());
    assert!(text(&refused).contains("creator"), "{}", text(&refused));
}
