//! The OCI sandbox against a *fake* container runtime: a shell script that records how it is called and
//! behaves as told. This is the one place a test double is acceptable: it checks the arguments the sandbox
//! builds, how it supervises and cleans up the runtime client (timeouts, output caps, stdin, cancellation,
//! ordering of `inspect`/`rm`), and how it reacts to what a runtime says. It proves **nothing** about what a
//! real runtime accepts or enforces: that is `tests/oci_real.rs`, which runs against real Docker and Podman.
#![cfg(target_os = "linux")] // the namespace and container sandboxes are exercised on Linux only

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ag_sandbox::{ExitStatus, IsolationLevel, OciConfig, OciSandbox, Sandbox, SandboxError};
use common::*;

const IMAGE: &str = "registry.example/check@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const FAKE: &str = r#"#!/bin/sh
dir=$(dirname "$0")
{ echo CALL; for a in "$@"; do printf 'A:%s\n' "$a"; done; } >> "$dir/calls.log"
case "$1" in
  run)
    env > "$dir/client-env.txt"
    prev=""
    for a in "$@"; do
      [ "$prev" = "--env-file" ] && cp "$a" "$dir/envfile.copy"
      prev="$a"
    done
    case "$(cat "$dir/mode" 2>/dev/null || echo ok)" in
      ok) echo "hello from the container"; echo "a warning" >&2; exit "$(cat "$dir/exit" 2>/dev/null || echo 0)" ;;
      stdin) cat ;;
      timeout) while [ ! -f "$dir/killed" ]; do sleep 0.02; done; exit 137 ;;
      slowcreate) sleep 0.8; touch "$dir/created"; while [ ! -f "$dir/killed" ]; do sleep 0.02; done; exit 137 ;;
      flood) while :; do echo yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy; done ;;
      error125) echo "docker: Error response from daemon: pull access denied for check" >&2; exit 125 ;;
    esac ;;
  kill) touch "$dir/killed" ;;
  inspect)
    case "$*" in
      # the state word a kill waits for: running until the container was killed
      *--format*)
        if [ "$(cat "$dir/mode")" = slowcreate ] && [ ! -f "$dir/created" ]; then echo "Error: No such container" >&2; exit 1; fi
        if [ -f "$dir/killed" ] || [ "$(cat "$dir/mode")" = ok ]; then echo exited; else echo running; fi ;;
      *) cat "$dir/inspect.json" 2>/dev/null || { echo "Error: No such container" >&2; exit 1; } ;;
    esac ;;
  info) cat "$dir/info.json" 2>/dev/null || echo '{}' ;;
  image) if [ -f "$dir/noimage" ]; then echo "Error: No such image" >&2; exit 1; else echo sha256:abc; fi ;;
  ps) cat "$dir/ps.txt" 2>/dev/null ;;
  rm) touch "$dir/removed" ;;
esac
"#;

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(mode: &str) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-docker");
        std::fs::write(&script, FAKE).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = Fake { dir };
        f.set("mode", mode);
        f
    }

    fn set(&self, file: &str, content: &str) {
        std::fs::write(self.dir.path().join(file), content).unwrap();
    }

    fn runtime(&self) -> PathBuf {
        self.dir.path().join("fake-docker")
    }

    fn sandbox(&self) -> OciSandbox {
        OciSandbox::new(self.runtime().to_str().unwrap(), IMAGE).unwrap()
    }

    /// Every call the sandbox made, as lists of arguments.
    fn calls(&self) -> Vec<Vec<String>> {
        let log = std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default();
        let mut calls: Vec<Vec<String>> = Vec::new();
        for line in log.lines() {
            if line == "CALL" {
                calls.push(Vec::new());
            } else if let (Some(a), Some(c)) = (line.strip_prefix("A:"), calls.last_mut()) {
                c.push(a.to_string());
            }
        }
        calls
    }

    fn call(&self, sub: &str) -> Option<Vec<String>> {
        self.calls().into_iter().find(|c| c.first().map(String::as_str) == Some(sub))
    }
}

fn flag_value(call: &[String], flag: &str) -> Option<String> {
    call.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn spec_in(dir: &Path) -> ag_sandbox::SandboxSpec {
    ag_sandbox::SandboxSpec::new(["sh", "-c", "run-the-check"], dir).env("CHECK_ENV", "visible-in-the-container")
}

#[test]
fn a_run_builds_the_expected_command_and_cleans_up_after_itself() {
    let fake = Fake::new("ok");
    let w = workdir();
    let o = fake.sandbox().run(&spec_in(w.path())).unwrap();
    assert_eq!(o.status, ExitStatus::Exited(0));
    assert_eq!(o.stdout_lossy(), "hello from the container\n");
    assert_eq!(o.stderr_lossy(), "a warning\n");

    let run = fake.call("run").expect("run was called");
    let name = flag_value(&run, "--name").unwrap();
    assert!(name.starts_with("ag-sandbox-"));
    assert_eq!(flag_value(&run, "--network").as_deref(), Some("none"));
    assert_eq!(flag_value(&run, "--cap-drop").as_deref(), Some("ALL"));
    assert_eq!(flag_value(&run, "--security-opt").as_deref(), Some("no-new-privileges"));
    assert!(run.contains(&"--read-only".to_string()));
    assert_eq!(flag_value(&run, "--workdir").as_deref(), Some("/work"));
    assert!(run.iter().any(|a| a == &format!("type=bind,src={},dst=/work", w.path().display())));
    let i = run.iter().position(|a| a == IMAGE).unwrap();
    assert_eq!(&run[i + 1..], ["-c", "run-the-check"]);
    assert_eq!(flag_value(&run, "--entrypoint").as_deref(), Some("sh"));

    // the container is inspected for an OOM kill, then force-removed, by the name it was started with
    assert_eq!(fake.call("inspect").unwrap().last(), Some(&name));
    let rm = fake.call("rm").unwrap();
    assert_eq!(rm, ["rm", "-f", "-v", &name]);

    // the command's environment travelled in a private file, not on any command line
    let calls = fake.calls();
    assert!(!calls.iter().flatten().any(|a| a.contains("visible-in-the-container")));
    let env_file = flag_value(&run, "--env-file").unwrap();
    assert_eq!(
        std::fs::read_to_string(fake.dir.path().join("envfile.copy")).unwrap(),
        "CHECK_ENV=visible-in-the-container\n"
    );
    assert!(!Path::new(&env_file).exists(), "the environment file is removed afterwards");
    // ... and the runtime client itself got a scrubbed environment
    let client_env = std::fs::read_to_string(fake.dir.path().join("client-env.txt")).unwrap();
    assert!(!client_env.contains("CHECK_ENV") && !client_env.contains("CARGO_MANIFEST_DIR"), "{client_env}");
}

#[test]
fn the_report_claims_nothing_the_runtime_did_not_record() {
    let fake = Fake::new("ok");
    let w = workdir();
    let sb = fake.sandbox();
    assert_eq!(sb.isolation(), IsolationLevel::Container);
    assert_eq!(sb.name(), "oci");
    // this runtime cannot say anything about the container (no record, no info): flags are not assumed
    let o = sb.run(&spec_in(w.path())).unwrap();
    let r = &o.isolation;
    assert!(!r.verified, "flags passed to a runtime are not observations");
    assert!(!(r.net_ns || r.private_root || r.read_only_root || r.no_new_privs || r.capabilities_dropped || r.seccomp));
    assert_eq!(r.image.as_deref(), Some(IMAGE));
    assert!(r.runtime.as_deref().unwrap().ends_with("fake-docker"));
    assert!(r.notes.iter().any(|n| n.contains("could not be read")), "{:?}", r.notes);
    // another image is another environment
    let other = OciSandbox::new(fake.runtime().to_str().unwrap(), IMAGE.replace("0123", "4567"))
        .unwrap()
        .run(&spec_in(w.path()))
        .unwrap();
    assert_ne!(o.environment_digest, other.environment_digest);
}

#[test]
fn a_record_that_contradicts_the_request_means_no_verdict() {
    let fake = Fake::new("ok");
    // the runtime says the container is privileged and not read-only: not what was asked for
    fake.set(
        "inspect.json",
        r#"[{"State":{"Status":"exited","ExitCode":0,"OOMKilled":false},"HostConfig":{"Privileged":true,"ReadonlyRootfs":false}}]"#,
    );
    let w = workdir();
    let err = fake.sandbox().run(&spec_in(w.path())).unwrap_err();
    match &err {
        SandboxError::Runtime(m) => assert!(m.contains("privileged") && m.contains("read-only root"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(fake.call("rm").is_some(), "the container is removed all the same");
}

#[test]
fn a_kill_that_podman_does_not_record_as_a_memory_kill_is_explained_in_the_notes() {
    let fake = Fake::new("ok");
    fake.set("exit", "137");
    fake.set("inspect.json", r#"[{"State":{"Status":"exited","ExitCode":137,"OOMKilled":false}}]"#);
    let w = workdir();
    let mut spec = spec_in(w.path());
    spec.limits.memory_bytes = 64 << 20;
    let excuse =
        |o: &ag_sandbox::Outcome| o.isolation.notes.iter().any(|n| n.contains("cannot be told from another SIGKILL"));
    // Podman's `info` and a record that says nothing about memory: the status is the exit status, and why
    // it is not a memory-limit status is said
    fake.set("info.json", r#"{"host":{"security":{"rootless":false,"seccompEnabled":true}}}"#);
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::Exited(137));
    assert!(excuse(&o), "{:?}", o.isolation.notes);
    // Docker's record of the same exit is no excuse: it would have said so
    fake.set("info.json", r#"{"SecurityOptions":[]}"#);
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::Exited(137));
    assert!(!excuse(&o), "{:?}", o.isolation.notes);
    // and a Podman that does record the kill gets the memory-limit status and no excuse
    fake.set("info.json", r#"{"host":{"security":{"rootless":false,"seccompEnabled":true}}}"#);
    fake.set("inspect.json", r#"[{"State":{"Status":"exited","ExitCode":137,"OOMKilled":true}}]"#);
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::MemoryLimit);
    assert!(!excuse(&o));
}

#[test]
fn the_runtimes_description_of_itself_is_used_for_seccomp_and_user_namespaces() {
    let fake = Fake::new("ok");
    fake.set("info.json", r#"{"SecurityOptions":["name=seccomp,profile=builtin","name=rootless"]}"#);
    let w = workdir();
    let o = fake.sandbox().run(&spec_in(w.path())).unwrap();
    assert!(o.isolation.seccomp && o.isolation.user_ns && !o.isolation.host_root);
    fake.set("info.json", r#"{"SecurityOptions":[]}"#);
    let o = fake.sandbox().run(&spec_in(w.path())).unwrap();
    assert!(!o.isolation.seccomp && !o.isolation.user_ns);
    assert!(o.isolation.notes.iter().any(|n| n.contains("no seccomp profile")), "{:?}", o.isolation.notes);
}

#[test]
fn the_commands_own_exit_status_is_passed_through() {
    let fake = Fake::new("ok");
    fake.set("exit", "42");
    let w = workdir();
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::Exited(42));
}

#[test]
fn a_timeout_stops_the_container_not_just_the_client() {
    let fake = Fake::new("timeout");
    let w = workdir();
    let mut spec = spec_in(w.path());
    spec.limits.wall_ms = 400;
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::TimedOut);
    let name = flag_value(&fake.call("run").unwrap(), "--name").unwrap();
    assert_eq!(fake.call("kill").expect("the runtime was asked to kill the container"), ["kill", &name]);
    assert!(fake.call("rm").is_some(), "and to remove it");
    assert!(host_processes_matching(&fake.runtime().to_string_lossy()).is_empty(), "no client process is left");
}

#[test]
fn a_timeout_during_the_creation_waits_for_the_container_instead_of_orphaning_it() {
    // The first container on a cold daemon takes a while to be created. Killing the client at that moment
    // leaves a container that was created and never started (found on a real Docker): the sandbox waits for it
    // to exist, kills it through the runtime, and only then lets go of the client.
    let fake = Fake::new("slowcreate");
    let w = workdir();
    let mut spec = spec_in(w.path());
    spec.limits.wall_ms = 300;
    let started = std::time::Instant::now();
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::TimedOut);
    assert!(fake.dir.path().join("created").exists(), "the container came into existence while the sandbox waited");
    let name = flag_value(&fake.call("run").unwrap(), "--name").unwrap();
    assert_eq!(fake.call("kill").expect("it was killed through the runtime"), ["kill", &name]);
    assert!(fake.call("rm").is_some());
    assert!(started.elapsed() < std::time::Duration::from_secs(8), "{:?}", started.elapsed());
}

#[test]
fn flooding_the_output_ends_the_container() {
    let fake = Fake::new("flood");
    let w = workdir();
    let mut spec = spec_in(w.path());
    spec.limits.output_bytes = 8192;
    let o = fake.sandbox().run(&spec).unwrap();
    assert_eq!(o.status, ExitStatus::OutputLimit);
    assert_eq!(o.stdout.len(), 8192);
    assert!(fake.call("kill").is_some() && fake.call("rm").is_some());
}

#[test]
fn stdin_reaches_the_container() {
    let fake = Fake::new("stdin");
    let w = workdir();
    let o = fake.sandbox().run(&spec_in(w.path()).stdin(b"through the pipe\n".to_vec())).unwrap();
    assert_eq!(o.stdout_lossy(), "through the pipe\n");
    assert!(fake.call("run").unwrap().contains(&"-i".to_string()));
}

#[test]
fn a_runtime_failure_is_an_error_not_a_failed_check() {
    let fake = Fake::new("error125");
    let w = workdir();
    let err = fake.sandbox().run(&spec_in(w.path())).unwrap_err();
    assert!(matches!(&err, SandboxError::Runtime(m) if m.contains("pull access denied")), "{err}");
    assert!(fake.call("rm").is_some(), "a half-created container is removed");
}

#[test]
fn an_out_of_memory_kill_is_reported_as_the_memory_limit() {
    let fake = Fake::new("ok");
    fake.set("exit", "137");
    fake.set("inspect.json", r#"[{"State":{"Status":"exited","ExitCode":137,"OOMKilled":true},"HostConfig":{}}]"#);
    let w = workdir();
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::MemoryLimit);
    // a 137 without the OOM flag is just the command's exit status
    fake.set("inspect.json", r#"[{"State":{"Status":"exited","ExitCode":137,"OOMKilled":false},"HostConfig":{}}]"#);
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::Exited(137));
    // a 137 and no readable record: no verdict is signed on a guess
    std::fs::remove_file(fake.dir.path().join("inspect.json")).unwrap();
    assert!(matches!(fake.sandbox().run(&spec_in(w.path())), Err(SandboxError::Runtime(_))));
}

#[test]
fn a_command_that_never_started_is_an_exec_error_and_one_that_exits_125_is_a_failed_check() {
    let fake = Fake::new("ok");
    let w = workdir();
    fake.set("exit", "127");
    fake.set(
        "inspect.json",
        r#"[{"State":{"Status":"created","ExitCode":127,"Error":"exec: \"run-the-check\": executable file not found in $PATH"},"HostConfig":{}}]"#,
    );
    let err = fake.sandbox().run(&spec_in(w.path())).unwrap_err();
    assert!(matches!(&err, SandboxError::Exec { program, .. } if program == "sh"), "{err}");
    // the command itself exited 125 after printing what looks like a runtime error: still its own status
    fake.set("exit", "125");
    fake.set("inspect.json", r#"[{"State":{"Status":"exited","ExitCode":125},"HostConfig":{}}]"#);
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::Exited(125));
}

#[test]
fn a_cancelled_run_kills_and_removes_the_container_and_returns_no_outcome() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fake = Fake::new("timeout");
    let w = workdir();
    let stop = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let err = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            stop.store(true, Ordering::SeqCst);
        });
        fake.sandbox().run_cancellable(&spec_in(w.path()), &|| stop.load(Ordering::SeqCst)).unwrap_err()
    });
    assert!(matches!(err, SandboxError::Cancelled), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let name = flag_value(&fake.call("run").unwrap(), "--name").unwrap();
    assert_eq!(fake.call("kill").expect("the runtime was asked to kill the container"), ["kill", &name]);
    assert!(fake.call("rm").is_some(), "and to remove it");
    assert!(host_processes_matching(&fake.runtime().to_string_lossy()).is_empty(), "no client process is left");
}

#[test]
fn the_image_and_the_daemon_can_be_checked_before_any_job() {
    let fake = Fake::new("ok");
    let sb = fake.sandbox();
    assert!(sb.image_present().is_ok() && sb.preflight().is_ok());
    fake.set("noimage", "");
    let err = sb.preflight().unwrap_err();
    match &err {
        SandboxError::Runtime(m) => {
            assert!(m.contains("never pulls") && m.contains(IMAGE) && m.contains(" pull "), "{m}")
        }
        other => panic!("{other:?}"),
    }
    fake.set(
        "info.json",
        r#"{"ServerVersion":"27.1.0","SecurityOptions":["name=seccomp,profile=builtin","name=rootless"]}"#,
    );
    let info = sb.probe().unwrap();
    assert_eq!(info.version.as_deref(), Some("27.1.0"));
    assert_eq!((info.rootless, info.seccomp, info.user_ns), (Some(true), Some(true), Some(true)));
    // a runtime that is not there says so
    let gone = OciSandbox::new("/definitely/not/a/runtime", IMAGE).unwrap();
    assert!(matches!(gone.probe(), Err(SandboxError::Unavailable { sandbox: "oci", .. })));
}

#[test]
fn orphans_of_dead_workers_are_removed_and_those_of_live_ones_are_not() {
    let fake = Fake::new("ok");
    let me = std::process::id();
    // pid 2^30 + 1 cannot exist (pid_max is at most 2^22)
    fake.set("ps.txt", &format!("ag-sandbox-1073741825-0-aa\nag-sandbox-{me}-0-bb\nunrelated\n"));
    let removed = fake.sandbox().reap_orphans().unwrap();
    assert_eq!(removed, ["ag-sandbox-1073741825-0-aa"]);
    let rm: Vec<Vec<String>> = fake.calls().into_iter().filter(|c| c[0] == "rm").collect();
    assert_eq!(rm.len(), 1);
    assert_eq!(rm[0].last().unwrap(), "ag-sandbox-1073741825-0-aa");
    // the listing is restricted to containers of this sandbox and this host
    let ps = fake.call("ps").unwrap();
    assert!(ps.iter().any(|a| a == "label=ag-sandbox=1"), "{ps:?}");
}

#[test]
fn a_missing_runtime_is_unavailable() {
    let w = workdir();
    let sb = OciSandbox::new("/definitely/not/a/runtime", IMAGE).unwrap();
    assert!(!sb.availability().is_available());
    let err = sb.run(&spec_in(w.path())).unwrap_err();
    assert!(matches!(err, SandboxError::Unavailable { sandbox: "oci", .. }), "{err}");
}

#[test]
fn unpinned_images_need_an_explicit_opt_in() {
    assert!(OciSandbox::new("docker", "ubuntu:24.04").is_err());
    let mut cfg = OciConfig::new("docker", "ubuntu:24.04");
    cfg.allow_unpinned_image = true;
    let sb = OciSandbox::with_config(cfg).unwrap();
    let w = workdir();
    // the report says plainly that the image is mutable
    let fake = Fake::new("ok");
    let mut cfg = OciConfig::new(fake.runtime().to_str().unwrap(), "ubuntu:24.04");
    cfg.allow_unpinned_image = true;
    let o = OciSandbox::with_config(cfg).unwrap().run(&spec_in(w.path())).unwrap();
    assert!(o.isolation.notes.iter().any(|n| n.contains("NOT pinned")));
    let _ = sb;
}
