//! The OCI sandbox against a *fake* container runtime: a shell script that records how it is called and
//! behaves as told. This checks the arguments the sandbox builds, how it supervises and cleans up the
//! runtime client, and how runtime failures are told apart from the command's own.
//!
//! **No real Docker or Podman exists on the machine these tests were written on, so nothing here shows that
//! a real runtime accepts these arguments or enforces what they request.**

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
      flood) while :; do echo yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy; done ;;
      error125) echo "docker: Error response from daemon: pull access denied for check" >&2; exit 125 ;;
    esac ;;
  kill) touch "$dir/killed" ;;
  inspect) cat "$dir/oom" 2>/dev/null || echo false ;;
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
fn the_report_says_what_was_requested_and_that_it_was_not_verified() {
    let fake = Fake::new("ok");
    let w = workdir();
    let sb = fake.sandbox();
    assert_eq!(sb.isolation(), IsolationLevel::Container);
    assert_eq!(sb.name(), "oci");
    let o = sb.run(&spec_in(w.path())).unwrap();
    let r = &o.isolation;
    assert!(!r.verified, "flags passed to a runtime are not observations");
    assert!(r.net_ns && r.private_root && r.read_only_root && r.no_new_privs && r.capabilities_dropped && r.seccomp);
    assert_eq!(r.image.as_deref(), Some(IMAGE));
    assert!(r.runtime.as_deref().unwrap().ends_with("fake-docker"));
    assert!(r.notes.iter().any(|n| n.contains("not observed")));
    // another image is another environment
    let other = OciSandbox::new(fake.runtime().to_str().unwrap(), IMAGE.replace("0123", "4567"))
        .unwrap()
        .run(&spec_in(w.path()))
        .unwrap();
    assert_ne!(o.environment_digest, other.environment_digest);
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
    assert!(host_processes_matching("fake-docker").is_empty(), "no client process is left");
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
    fake.set("oom", "true");
    let w = workdir();
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::MemoryLimit);
    // a 137 without the OOM flag is just the command's exit status
    fake.set("oom", "false");
    assert_eq!(fake.sandbox().run(&spec_in(w.path())).unwrap().status, ExitStatus::Exited(137));
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
