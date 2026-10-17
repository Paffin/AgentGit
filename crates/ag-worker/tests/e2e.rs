//! The execution plane end to end: a real `ag-server` (in process, over real TCP and gRPC), a real worker
//! running real commands in a real sandbox, real signatures checked by the real policy engine.
//!
//! The sandbox is the namespace sandbox when this machine can run it. Otherwise the tests fall back to the
//! limits-only process sandbox (explicitly opted into, as the worker requires) so that the claim / check out /
//! run / sign / complete loop is still exercised; the tests that are *about* isolation then skip.
#![allow(missing_docs)]
#![cfg(unix)] // the execution plane drives a Unix sandbox; tests/unsupported.rs covers other platforms

mod common;

use std::sync::Arc;
use std::time::Duration;

use ag_crypto::KeyPair;
use ag_policy::CheckResult;
use ag_protocol::v1;
use ag_server::authz::Role;
use ag_worker::{Disposition, Options, Worker};
use common::*;
use serde_json::{Value, json};

// ---- tests -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_runs_the_policys_check_on_the_exact_tree_and_unblocks_the_integration() {
    let w = World::new(sh("test -f lib.rs && grep -q 'fn main' lib.rs && test \"$AG_CHECK\" = tests", 60)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"fn main() {}\n"), ("sub/dir/data.txt", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    assert_eq!(jobs.len(), 1);
    // The policy gates the integration until a trusted attestation exists.
    assert_eq!(w.integrate(&pid).await, "blocked");

    let report = only(w.run_worker(w.profile(), opts()).await);
    assert_eq!(report.job_id, jobs[0].id);
    let Disposition::Attested { result, attestation } = &report.disposition else { panic!("{report:?}") };
    assert_eq!(*result, CheckResult::Pass, "{report:?}");

    // The job is done, the result says how it went, and the attestation is the one that was signed.
    let job = w.job(&report.job_id).await;
    assert_eq!(job.state, "succeeded", "{job:?}");
    assert_eq!(job.attempts, 1);
    let result: Value = serde_json::from_str(&job.result_json).unwrap();
    assert_eq!(result["outcome"], "pass");
    assert_eq!(result["attestation"], attestation.as_str());

    // The same tree is now validated by a trusted issuer: the integration goes through.
    assert_eq!(w.integrate(&pid).await, "integrated");
    // Nothing is left behind in the scratch directory.
    let leftovers = std::fs::read_dir(&w.scratch).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "job directories are removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_check_is_a_signed_fail_that_keeps_the_integration_blocked() {
    let w = World::new(sh("echo 'compiling...'; echo 'error: nope' >&2; exit 3", 60)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"broken\n")]);
    let jobs = w.queue_validation(&pid).await;

    let report = only(w.run_worker(w.profile(), opts()).await);
    let Disposition::Attested { result, .. } = &report.disposition else { panic!("{report:?}") };
    assert_eq!(*result, CheckResult::Fail);
    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "succeeded", "a failed check is a completed job with a `fail` verdict");
    let r: Value = serde_json::from_str(&job.result_json).unwrap();
    assert_eq!(r["outcome"], "fail");
    // The worker's own account of the run is stored with the job.
    assert_eq!(r["status"], json!({"exited": 3}), "{r}");
    assert!(r["stdout"]["tail"].as_str().unwrap().contains("compiling"), "{r}");
    assert!(r["stderr"]["tail"].as_str().unwrap().contains("error: nope"), "{r}");
    // A trusted `fail` is a refusal ("denied"), not merely a missing validation ("blocked").
    assert_eq!(w.integrate(&pid).await, "denied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_out_of_time_is_a_verdict_not_an_infrastructure_error() {
    let w = World::new(sh("sleep 30", 1)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let started = std::time::Instant::now();
    let report = only(w.run_worker(w.profile(), opts()).await);
    assert!(started.elapsed() < Duration::from_secs(20), "the policy's 1 s limit was enforced");
    assert!(matches!(report.disposition, Disposition::Attested { result: CheckResult::Fail, .. }), "{report:?}");
    let r: Value = serde_json::from_str(&w.job(&jobs[0].id).await.result_json).unwrap();
    assert_eq!(r["status"], json!("timed_out"));
    assert_eq!(w.integrate(&pid).await, "denied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_the_worker_cannot_run_fails_the_job_and_signs_nothing() {
    let w = World::new(json!({"argv": ["definitely-not-installed-anywhere"], "timeout_secs": 30})).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(w.profile(), opts()).await);
    let Disposition::Failed { error, retry } = &report.disposition else { panic!("{report:?}") };
    assert!(*retry, "another worker (with the toolchain) may succeed");
    assert!(error.contains("definitely-not-installed-anywhere"), "{error}");
    // The job is back in the queue with the reason; no attestation exists, so the tree is not blamed.
    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "queued", "{job:?}");
    assert!(job.error.contains("definitely-not-installed-anywhere"), "{job:?}");
    assert_eq!(w.integrate(&pid).await, "blocked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_worker_will_not_do_it_refuses_instead_of_trimming() {
    // The policy asks for the network; this worker does not allow it.
    let w = World::new(json!({"argv": ["true"], "timeout_secs": 30, "network": true})).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(w.profile(), opts()).await);
    let Disposition::Failed { error, retry: true } = &report.disposition else { panic!("{report:?}") };
    assert!(error.contains("network"), "{error}");
    assert_eq!(w.job(&jobs[0].id).await.state, "queued");

    // ... and a request above the worker's ceiling.
    let w = World::new(json!({"argv": ["true"], "timeout_secs": 300})).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    w.queue_validation(&pid).await;
    let mut profile = w.profile();
    profile.ceilings.timeout_secs = 60;
    let report = only(w.run_worker(profile, opts()).await);
    let Disposition::Failed { error, retry: true } = &report.disposition else { panic!("{report:?}") };
    assert!(error.contains("timeout_secs") && error.contains("60"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_candidate_that_moved_on_completes_the_job_as_stale() {
    let w = World::new(sh("true", 30)).await;
    let (pid, head) = w.propose(&[("lib.rs", b"v1\n")]);
    let jobs = w.queue_validation(&pid).await;
    // The author pushes a new head before any worker looks: the job's tree is no longer the candidate.
    let newer = w.commit(Some(head), &[("lib.rs", b"v2\n")]);
    w.srv.repo().update_proposal(&pid, &newer, &id("user:alice")).unwrap();

    let report = only(w.run_worker(w.profile(), opts()).await);
    assert_eq!(report.disposition, Disposition::Stale, "{report:?}");
    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "succeeded");
    let r: Value = serde_json::from_str(&job.result_json).unwrap();
    assert_eq!(r["stale"], true);
    // No attestation for either tree: the policy still blocks.
    assert_eq!(w.integrate(&pid).await, "blocked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_check_keeps_its_claim_alive_by_heartbeat() {
    let w = World::new(sh("sleep 4", 30)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    // A lease far shorter than the check: without heartbeats the job would be handed out again.
    let o = Options { lease: Duration::from_millis(1500), ..opts() };
    let report = only(w.run_worker(w.profile(), o).await);
    assert!(matches!(report.disposition, Disposition::Attested { result: CheckResult::Pass, .. }), "{report:?}");
    let job = w.job(&jobs[0].id).await;
    assert_eq!((job.state.as_str(), job.attempts), ("succeeded", 1), "{job:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_policy_can_pin_the_environment_and_a_worker_in_another_environment_does_not_attest() {
    let w = World::with_policy(
        sh("true", 30),
        Some("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
    )
    .await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(w.profile(), opts()).await);
    let Disposition::Failed { error, retry: true } = &report.disposition else { panic!("{report:?}") };
    assert!(error.contains("environment digest") && error.contains("sha256:0000"), "{error}");
    assert_eq!(w.job(&jobs[0].id).await.state, "queued");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_environment_digest_in_the_attestation_is_the_sandboxs_and_can_be_pinned() {
    // First run: learn the digest this worker produces. Second run: a policy that pins it accepts the worker.
    let w = World::new(sh("true", 30)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    only(w.run_worker(w.profile(), opts()).await);
    let r: Value = serde_json::from_str(&w.job(&jobs[0].id).await.result_json).unwrap();
    let digest = r["environment_digest"].as_str().unwrap().to_string();
    assert!(digest.starts_with("sha256:") && digest.len() == 71, "{digest}");

    let w2 = World::with_policy(sh("true", 30), Some(&digest)).await;
    let (pid2, _) = w2.propose(&[("lib.rs", b"different tree, same environment\n")]);
    w2.queue_validation(&pid2).await;
    let report = only(w2.run_worker(w2.profile(), opts()).await);
    assert!(matches!(report.disposition, Disposition::Attested { result: CheckResult::Pass, .. }), "{report:?}");
    assert_eq!(w2.integrate(&pid2).await, "integrated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_check_cannot_see_the_workers_key_or_run_as_the_worker() {
    if !namespace_works() {
        eprintln!("SKIP: needs the namespace sandbox");
        return;
    }
    let keyfile = tempfile::tempdir().unwrap();
    let key_path = keyfile.path().join("worker-key.json");
    KeyPair::generate().unwrap().save(&key_path).unwrap();
    std::fs::set_permissions(keyfile.path(), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let script = r#"
        test ! -e "$KEYFILE" || { echo "the worker's key is visible: $KEYFILE"; exit 10; }
        test "$(id -u)" = 1000 || { echo "unexpected uid $(id -u)"; exit 11; }
        test -z "$AG_TOKEN" || { echo "credential in the environment"; exit 12; }
        test -r lib.rs || { echo "cannot read the tree"; exit 13; }
        echo ok > result.txt || { echo "cannot write the tree"; exit 14; }
    "#;
    let mut run = sh(script, 30);
    run["env"] = json!({"KEYFILE": key_path.to_str().unwrap()});
    let w = World::new(run).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(w.profile(), opts()).await);
    let job = w.job(&jobs[0].id).await;
    assert!(
        matches!(report.disposition, Disposition::Attested { result: CheckResult::Pass, .. }),
        "{report:?}\n{}",
        job.result_json
    );
    let r: Value = serde_json::from_str(&job.result_json).unwrap();
    assert_eq!(r["sandbox"], "namespace");
    assert_eq!(r["isolation_verified"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_with_a_key_the_policy_does_not_trust_runs_checks_that_count_for_nothing() {
    let w = World::new(sh("true", 30)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    w.queue_validation(&pid).await;
    let ci = w.srv.client("service:ci-1", Role::Runner).await;
    let stranger = KeyPair::generate().unwrap();
    let worker = Arc::new(Worker::new(ci, w.profile(), stranger, id("service:ci-1"), opts()));
    let (_stop, rx) = tokio::sync::watch::channel(false);
    let report = only(worker.run(rx, true).await.unwrap());
    // The server accepts and stores the attestation (a job is only a delivery mechanism) ...
    assert!(matches!(report.disposition, Disposition::Attested { result: CheckResult::Pass, .. }), "{report:?}");
    // ... but the policy does not trust that key, so it does not count.
    assert_eq!(w.integrate(&pid).await, "blocked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_lanes_run_several_checks_at_once() {
    let checks = vec![
        json!({"check": "lint", "run": sh("sleep 2; test -f lib.rs", 30)}),
        json!({"check": "tests", "run": sh("sleep 2; grep -q main lib.rs", 30)}),
        json!({"check": "docs", "run": sh("sleep 2; test -d sub", 30)}),
    ];
    let w = World::with_checks(checks, None).await;
    let (pid, _) = w.propose(&[("lib.rs", b"fn main() {}\n"), ("sub/x.txt", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    assert_eq!(jobs.len(), 3);

    let started = std::time::Instant::now();
    let reports = w.run_worker(w.profile(), Options { concurrency: 3, ..opts() }).await;
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert!(
        reports.iter().all(|r| matches!(r.disposition, Disposition::Attested { result: CheckResult::Pass, .. })),
        "{reports:?}"
    );
    let mut checks: Vec<_> = reports.iter().map(|r| r.check.as_str()).collect();
    checks.sort_unstable();
    assert_eq!(checks, ["docs", "lint", "tests"]);
    assert!(started.elapsed() < Duration::from_secs(5), "three two-second checks ran in parallel, not in turn");
    // Each lane has its own name on the server, so one cannot finish another's job.
    let mut who: Vec<String> = Vec::new();
    for j in &jobs {
        who.push(w.job(&j.id).await.claimed_by);
    }
    who.sort();
    who.dedup();
    assert_eq!(who.len(), 3, "{who:?}");
    assert_eq!(w.integrate(&pid).await, "integrated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_worker_stops_promptly_when_told_to() {
    let w = World::new(sh("true", 30)).await;
    let ci = w.srv.client("service:ci-1", Role::Runner).await;
    let worker = Arc::new(Worker::new(
        ci,
        w.profile(),
        w.ci_key_clone(),
        id("service:ci-1"),
        // a long poll: stopping must not wait for it to run out
        Options { wait: Duration::from_secs(25), ..opts() },
    ));
    let (stop, rx) = tokio::sync::watch::channel(false);
    let run = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run(rx, false).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let asked = std::time::Instant::now();
    stop.send(true).unwrap();
    let reports = tokio::time::timeout(Duration::from_secs(5), run).await.expect("stopped in time").unwrap().unwrap();
    assert!(reports.is_empty());
    assert!(asked.elapsed() < Duration::from_secs(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_credential_without_the_runner_role_is_a_fatal_error_not_a_retry_loop() {
    let w = World::new(sh("true", 30)).await;
    let nobody = w.srv.client("user:nobody", Role::Reader).await;
    let worker = Arc::new(Worker::new(
        nobody,
        w.profile(),
        w.ci_key_clone(),
        id("user:nobody"),
        Options { repo: Some(REPO.into()), ..opts() },
    ));
    let (_stop, rx) = tokio::sync::watch::channel(false);
    let err = tokio::time::timeout(Duration::from_secs(10), worker.run(rx, false)).await.expect("gave up").unwrap_err();
    assert_eq!(err.code(), Some(ag_protocol::ErrorCode::PermissionDenied), "{err}");
}

// ---- the real binaries ---------------------------------------------------------------------------------

fn run_bin(bin: &str, args: &[String], envs: &[(String, String)]) -> std::process::Output {
    std::process::Command::new(bin)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn worker_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ag-worker")
}

async fn run_worker_bin(args: Vec<String>, envs: Vec<(String, String)>) -> std::process::Output {
    tokio::task::spawn_blocking(move || run_bin(worker_bin(), &args, &envs)).await.unwrap()
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// `--sandbox` flags matching what this machine can do (the process sandbox only when it must).
fn sandbox_flags() -> Vec<String> {
    if namespace_works() {
        strings(&["--sandbox", "namespace"])
    } else {
        strings(&["--sandbox", "process", "--insecure-allow-process-sandbox"])
    }
}

fn text(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn the_cli_makes_keys_checks_the_machine_and_refuses_unsafe_choices() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("worker.key");
    let k = key.to_str().unwrap();

    let out = run_bin(worker_bin(), &strings(&["keygen", "--out", k, "--identity", "service:ci-1"]), &[]);
    assert!(out.status.success(), "{}", text(&out));
    let public = KeyPair::load(&key).unwrap().public().to_hex();
    let said = text(&out);
    assert!(
        said.contains(&public) && said.contains("\"identity\": \"service:ci-1\"") && said.contains("trusted_issuers"),
        "{said}"
    );
    assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
    // never overwrites a key
    let again = run_bin(worker_bin(), &strings(&["keygen", "--out", k]), &[]);
    assert!(!again.status.success());
    assert_eq!(KeyPair::load(&key).unwrap().public().to_hex(), public);

    let out = run_bin(worker_bin(), &strings(&["doctor"]), &[]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("process") && said.contains("available") && said.contains("--sandbox auto"), "{said}");

    // The limits-only sandbox is not isolation and must be asked for by its unflattering name.
    let url = "http://127.0.0.1:9";
    let out = run_bin(worker_bin(), &strings(&["run", "--server", url, "--key", k, "--sandbox", "process"]), &[]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("not isolation") && text(&out).contains("--insecure-allow-process-sandbox"),
        "{}",
        text(&out)
    );

    // A key other users can read is refused (the tooling for it is ag-crypto's, the policy is ours).
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut args = strings(&["run", "--server", url, "--key", k, "--insecure-allow-process-sandbox"]);
    args.extend(strings(&["--sandbox", "process"]));
    let out = run_bin(worker_bin(), &args, &[("AG_TOKEN".into(), "x".into())]);
    assert!(!out.status.success());
    assert!(text(&out).contains("cannot load the signing key"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ag_worker_binary_claims_runs_signs_and_completes() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("worker.key");
    let k = key.to_str().unwrap().to_string();
    assert!(
        run_bin(worker_bin(), &strings(&["keygen", "--out", &k, "--identity", "service:ci-1"]), &[]).status.success()
    );
    let trusted = KeyPair::load(&key).unwrap();

    let check = json!({"check": "tests", "run": sh("test -f lib.rs && echo hello-from-the-check", 60)});
    let w = World::with_checks(vec![check], Some(&trusted)).await;
    let (pid, _) = w.propose(&[("lib.rs", b"fn main() {}\n")]);
    let jobs = w.queue_validation(&pid).await;
    assert_eq!(w.integrate(&pid).await, "blocked");

    let token = w.srv.token("service:ci-1", Role::Runner);
    let mut args =
        strings(&["run", "--server", &w.srv.url, "--key", &k, "--once", "--wait-secs", "10", "--label", "bin-test"]);
    args.extend(["--scratch-dir".to_string(), w.scratch.to_str().unwrap().to_string()]);
    args.extend(sandbox_flags());
    let out = run_worker_bin(args, vec![("AG_TOKEN".into(), token)]).await;
    assert!(out.status.success(), "{}", text(&out));

    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "succeeded", "{job:?}\n{}", text(&out));
    assert_eq!(job.claimed_by, "service:ci-1|bin-test", "the issuer was learned from the server");
    let r: Value = serde_json::from_str(&job.result_json).unwrap();
    assert!(r["stdout"]["tail"].as_str().unwrap().contains("hello-from-the-check"), "{r}");
    assert_eq!(w.integrate(&pid).await, "integrated");

    // A credential the server does not know is reported as such, not retried forever.
    let args = {
        let mut a = strings(&["run", "--server", &w.srv.url, "--key", &k, "--once", "--wait-secs", "1"]);
        a.extend(sandbox_flags());
        a
    };
    let out = run_worker_bin(args, vec![("AG_TOKEN".into(), "agt_0000000000000000_nonsense".into())]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("who this credential is"), "{}", text(&out));
}

/// The real `ag-server` binary: `AG_SERVER_BIN`, else the one cargo built next to the test binaries.
fn real_server_binary() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("AG_SERVER_BIN") {
        let p = std::path::PathBuf::from(p);
        assert!(p.is_file(), "AG_SERVER_BIN={} is not a file", p.display());
        return Some(p);
    }
    let sibling = std::env::current_exe().ok()?.parent()?.parent()?.join("ag-server");
    sibling.is_file().then_some(sibling)
}

struct Proc(std::process::Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_server_binary_and_the_real_worker_binary_agree() {
    let Some(server_bin) = real_server_binary() else {
        eprintln!("SKIP: no ag-server binary (build it with `cargo build -p ag-server` or set AG_SERVER_BIN)");
        return;
    };
    let server_bin = server_bin.to_str().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let key = dir.path().join("worker.key");
    let k = key.to_str().unwrap().to_string();
    assert!(
        run_bin(worker_bin(), &strings(&["keygen", "--out", &k, "--identity", "service:ci-1"]), &[]).status.success()
    );
    let trusted = KeyPair::load(&key).unwrap();

    // Seed a repository on disk with this crate's copy of the engine, then hand the directory to the real server.
    let proposal = {
        let mut cfg = ag_server::config::ServerConfig::default();
        cfg.server.data_dir = data.clone();
        let app = ag_server::app::App::with_options(cfg, Arc::new(ag_storage::SystemClock), None).unwrap();
        app.repos.create(REPO, "main", "", &id("system:test"), Default::default()).unwrap();
        let repo = app.repos.handle(REPO).unwrap();
        let policy = json!({
            "trusted_issuers": [{"identity": "service:ci-1", "public_key": trusted.public().to_hex()}],
            "required_validations": [{"check": "tests", "run": sh("grep -q 'fn main' lib.rs", 60)}]
        });
        repo.set_policy("main", &policy.to_string(), &id("user:root")).unwrap();
        let (pid, _) = propose_files(&repo, &[("lib.rs", b"fn main() {}\n")]);
        drop(repo);
        app.repos.close_all();
        pid
    };

    // Roles and credentials through the real binary's offline administration.
    let admin = |args: &[&str]| {
        let mut a = strings(&["admin", "--data-dir", data.to_str().unwrap()]);
        a.extend(strings(args));
        let o = run_bin(&server_bin, &a, &[]);
        assert!(o.status.success(), "ag-server admin {args:?}: {}", text(&o));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let mut tokens = std::collections::BTreeMap::new();
    for (who, role) in [("service:ci-1", "runner"), ("user:alice", "writer"), ("user:carol", "integrator")] {
        admin(&["grant", "--subject", who, "--repo", REPO, "--role", role]);
        tokens.insert(who, admin(&["mint", "--subject", who, "--ttl", "600"]));
    }

    // Start the server.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let listen = format!("127.0.0.1:{port}");
    let log_path = dir.path().join("server.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let _server = Proc(
        std::process::Command::new(&server_bin)
            .args(["serve", "--data-dir", data.to_str().unwrap(), "--listen", &listen])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let url = format!("http://{listen}");
    let connect = |who: &'static str| {
        let (url, token, log_path) = (url.clone(), tokens[who].clone(), log_path.clone());
        async move {
            let mut last = String::new();
            for _ in 0..200 {
                match ag_client::Client::connect(ag_client::Options {
                    endpoint: url.clone(),
                    token: token.clone(),
                    ..Default::default()
                })
                .await
                {
                    Ok(c) if c.negotiate().await.is_ok() => return c,
                    Ok(_) => last = "negotiate failed".into(),
                    Err(e) => last = e.to_string(),
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!("the server did not come up: {last}\n{log}");
        }
    };
    let alice = connect("user:alice").await;
    let carol = connect("user:carol").await;

    let req = v1::RequestValidationRequest { repo: REPO.into(), proposal: proposal.clone(), ..Default::default() };
    let jobs = alice.integration().request_validation(alice.req(req)).await.unwrap().into_inner().jobs;
    assert_eq!(jobs.len(), 1);
    let integrate = |c: &ag_client::Client| {
        let c = c.clone();
        let req = v1::IntegrateRequest { repo: REPO.into(), proposal: proposal.clone(), ..Default::default() };
        async move {
            c.integration()
                .integrate(c.req(req))
                .await
                .unwrap()
                .into_inner()
                .outcome
                .map(|o| o.status)
                .unwrap_or_default()
        }
    };
    assert_eq!(integrate(&carol).await, "blocked");

    let scratch = dir.path().join("scratch");
    let mut args = strings(&["run", "--server", &url, "--key", &k, "--once", "--wait-secs", "10", "--label", "real"]);
    args.extend(["--scratch-dir".to_string(), scratch.to_str().unwrap().to_string()]);
    args.extend(sandbox_flags());
    let out = run_worker_bin(args, vec![("AG_TOKEN".into(), tokens["service:ci-1"].clone())]).await;
    assert!(out.status.success(), "{}", text(&out));

    let job = alice
        .execution()
        .get_job(alice.req(v1::GetJobRequest { repo: REPO.into(), job_id: jobs[0].id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(job.state, "succeeded", "{job:?}\n{}", text(&out));
    assert_eq!(integrate(&carol).await, "integrated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_worker_can_ask_the_server_who_it_is() {
    let w = World::new(sh("true", 30)).await;
    let ci = w.srv.client("service:ci-1", Role::Runner).await;
    assert_eq!(Worker::whoami(&ci).await.unwrap().to_string(), "service:ci-1");
}
