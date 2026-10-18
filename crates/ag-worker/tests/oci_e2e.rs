//! The execution plane with a **real container runtime**: a real `ag-server` in this process, a real worker,
//! a real Docker or Podman running the policy's check in a container, real signatures.
//!
//! Needs the same environment as `crates/ag-sandbox/tests/oci_real.rs`:
//!
//! * `AG_OCI_RUNTIME` — `docker`, `podman` or a path (default `docker`);
//! * `AG_OCI_IMAGE` — a digest-pinned BusyBox image that is already in the runtime's local store;
//! * `AG_REQUIRE_OCI=1` — a host that cannot run them fails instead of skipping (CI sets it).
//!
//! The first test is the one that would have caught the container user not being able to write `/work` for a
//! worker that is not root: the check writes to the tree it was given.
#![allow(missing_docs)]
#![cfg(unix)]

mod common;

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_policy::CheckResult;
use ag_sandbox::{IsolationLevel, OciConfig, OciSandbox};
use ag_server::authz::Role;
use ag_worker::{Backend, Disposition, Profile, Worker};
use common::*;
use serde_json::{Value, json};

static LOCK: Mutex<()> = Mutex::new(());

struct Real {
    runtime: String,
    image: String,
    user: String,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn real(test: &str) -> Option<Real> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let required = std::env::var("AG_REQUIRE_OCI").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no"));
    let skip = |why: String| {
        assert!(!required, "AG_REQUIRE_OCI is set, but {test} cannot run on this host: {why}");
        let _ = writeln!(std::io::stderr(), "SKIP {test}: {why}");
        None
    };
    let runtime = std::env::var("AG_OCI_RUNTIME").ok().filter(|r| !r.trim().is_empty()).unwrap_or("docker".into());
    let Ok(image) = std::env::var("AG_OCI_IMAGE") else {
        return skip("AG_OCI_IMAGE (a digest-pinned BusyBox image in the local store) is not set".into());
    };
    let info = match ag_sandbox::probe_runtime(&runtime, Duration::from_secs(20)) {
        Ok(i) => i,
        Err(e) => return skip(format!("the runtime is not usable: {e}")),
    };
    let sandbox = OciSandbox::new(&runtime, &image).expect("a valid reference");
    if let Err(e) = sandbox.image_present() {
        return skip(format!("the image is not available: {e}"));
    }
    let (euid, egid) = ag_sandbox::effective_ids();
    let user = info.user_for_tree(euid, egid, "65534:65534");
    Some(Real { runtime, image, user, _guard: guard })
}

impl Real {
    /// A profile for a container worker. The image brings its own `/bin` and `/usr`: mounting the host's over
    /// them would run the host's programs in the image's file system (`ag-worker --no-system-mounts`).
    fn profile(&self, w: &World) -> Profile {
        let mut p = Profile::new(Backend::oci(&self.runtime, Some(self.image.clone()), &self.user));
        assert_eq!(p.min_isolation, IsolationLevel::Namespaces);
        p.ro_mounts.clear();
        p.scratch_dir = w.scratch.clone();
        p
    }

    fn run(&self, argv: &[&str]) -> Value {
        json!({"argv": argv, "timeout_secs": 60, "image": self.image})
    }

    fn containers(&self) -> Vec<String> {
        let out = std::process::Command::new(&self.runtime)
            .args(["ps", "-a", "--filter", "label=ag-sandbox=1", "--format", "{{.Names}}"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_string).collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_check_runs_in_a_real_container_on_the_exact_tree_and_can_write_it() {
    let Some(r) = real("a_check_runs_in_a_real_container_on_the_exact_tree_and_can_write_it") else { return };
    let script = r#"test -f lib.rs && grep -q 'fn main' lib.rs && test "$AG_CHECK" = tests && echo built > target.txt && test "$(cat target.txt)" = built"#;
    let w = World::new(r.run(&["sh", "-c", script])).await;
    let (pid, _) = w.propose(&[("lib.rs", b"fn main() {}\n"), ("sub/dir/data.txt", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    assert_eq!(w.integrate(&pid).await, "blocked");

    let report = only(w.run_worker(r.profile(&w), opts()).await);
    let Disposition::Attested { result, .. } = &report.disposition else { panic!("{report:?}") };
    assert_eq!(*result, CheckResult::Pass, "{report:?}\n{}", w.job(&jobs[0].id).await.result_json);

    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "succeeded", "{job:?}");
    let result: Value = serde_json::from_str(&job.result_json).unwrap();
    assert_eq!(result["sandbox"], "oci");
    assert_eq!(result["isolation_verified"], true, "every protection was read back from the runtime: {result}");
    assert_eq!(result["memory_enforcement"], "runtime");
    assert!(result["environment_digest"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(w.integrate(&pid).await, "integrated");
    assert_eq!(std::fs::read_dir(&w.scratch).map(|d| d.count()).unwrap_or(0), 0, "job directories are removed");
    assert!(r.containers().is_empty(), "{:?}", r.containers());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_check_in_a_container_is_a_signed_fail_with_the_checks_own_output() {
    let Some(r) = real("a_failing_check_in_a_container_is_a_signed_fail_with_the_checks_own_output") else { return };
    let w = World::new(r.run(&["sh", "-c", "echo compiling; echo 'error: nope' >&2; exit 3"])).await;
    let (pid, _) = w.propose(&[("lib.rs", b"broken\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(r.profile(&w), opts()).await);
    let Disposition::Attested { result, .. } = &report.disposition else { panic!("{report:?}") };
    assert_eq!(*result, CheckResult::Fail);
    let result: Value = serde_json::from_str(&w.job(&jobs[0].id).await.result_json).unwrap();
    assert_eq!(result["status"], json!({"exited": 3}), "{result}");
    assert!(result["stdout"]["tail"].as_str().unwrap().contains("compiling"));
    assert!(result["stderr"]["tail"].as_str().unwrap().contains("error: nope"));
    assert_eq!(w.integrate(&pid).await, "denied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_that_is_not_in_the_local_store_fails_the_job_before_the_tree_is_downloaded() {
    let Some(r) = real("an_image_that_is_not_in_the_local_store_fails_the_job_before_the_tree_is_downloaded") else {
        return;
    };
    let absent = format!("{}@sha256:{}", r.image.split('@').next().unwrap(), "0123456789abcdef".repeat(4));
    let w = World::new(json!({"argv": ["true"], "timeout_secs": 30, "image": absent})).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let report = only(w.run_worker(r.profile(&w), opts()).await);
    let Disposition::Failed { error, retry: true } = &report.disposition else { panic!("{report:?}") };
    assert!(error.contains("never pulls") && error.contains(&absent), "{error}");
    let job = w.job(&jobs[0].id).await;
    assert_eq!(job.state, "queued", "another worker that has the image may take it: {job:?}");
    assert_eq!(w.integrate(&pid).await, "blocked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_the_worker_now_kills_the_container_and_hands_the_job_back() {
    let Some(r) = real("stopping_the_worker_now_kills_the_container_and_hands_the_job_back") else { return };
    let w = World::new(r.run(&["sleep", "3000"])).await;
    let (pid, _) = w.propose(&[("lib.rs", b"x\n")]);
    let jobs = w.queue_validation(&pid).await;
    let ci = w.srv.client("service:ci-1", Role::Runner).await;
    let worker = Arc::new(Worker::new(ci, r.profile(&w), w.ci_key_clone(), id("service:ci-1"), opts()));
    let (_stop, rx) = tokio::sync::watch::channel(false);
    let running = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run(rx, true).await }
    });
    // wait until the check's container is really there
    let started = Instant::now();
    while r.containers().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(60), "the container never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let asked = Instant::now();
    worker.cancel();
    let reports =
        tokio::time::timeout(Duration::from_secs(60), running).await.expect("stopped in time").unwrap().unwrap();
    assert!(asked.elapsed() < Duration::from_secs(40), "{:?}", asked.elapsed());
    let report = only(reports);
    let Disposition::Failed { error, retry: true } = &report.disposition else { panic!("{report:?}") };
    assert!(error.contains("stopped"), "{error}");
    assert_eq!(w.job(&jobs[0].id).await.state, "queued", "the job is back in the queue at once");
    assert!(r.containers().is_empty(), "the container was killed and removed: {:?}", r.containers());
    // no attestation was signed for a check nobody saw finish
    assert_eq!(w.integrate(&pid).await, "blocked");
}

#[test]
fn the_sandbox_the_profile_builds_is_the_one_the_tests_above_exercise() {
    let Some(r) = real("the_sandbox_the_profile_builds_is_the_one_the_tests_above_exercise") else { return };
    let backend = Backend::oci(&r.runtime, Some(r.image.clone()), &r.user);
    let sb = backend.sandbox_for(None).unwrap();
    assert_eq!((sb.name(), sb.isolation()), ("oci", IsolationLevel::Container));
    sb.preflight().unwrap();
    // the image of the policy wins over the worker's default
    assert!(backend.sandbox_for(Some(&r.image)).is_ok());
    let cfg = OciConfig::new(&r.runtime, &r.image);
    assert_eq!(cfg.control_timeout, ag_sandbox::DEFAULT_CONTROL_TIMEOUT);
}
