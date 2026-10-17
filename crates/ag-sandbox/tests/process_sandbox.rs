//! The limits-only fallback: what it does enforce, and — as importantly — what it does not (it is not
//! isolation, and these tests say so out loud).
#![allow(unsafe_code)]
#![cfg(unix)]
// the sandbox crate is empty on other platforms (Windows has no equivalent; docs/operations/windows.md)
// The tests that rely on Linux details (/proc, RLIMIT_DATA, setsid, ...) are Linux-only; the rest run anywhere.
#![cfg_attr(not(target_os = "linux"), allow(unused_imports, dead_code))]

mod common;

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use ag_sandbox::{Enforcement, ExitStatus, IsolationLevel, Limits, Sandbox};
use common::*;

fn p() -> ag_sandbox::ProcessSandbox {
    process()
}

#[test]
fn it_reports_that_it_is_not_isolation() {
    let w = workdir();
    let o = run(&p(), &probe_spec_for(&p(), w.path(), &["ids"]));
    assert_eq!(p().isolation(), IsolationLevel::None);
    assert_eq!(p().name(), "process");
    let r = &o.isolation;
    assert!(r.verified && r.rlimits);
    assert!(!(r.user_ns || r.mount_ns || r.pid_ns || r.net_ns || r.ipc_ns || r.uts_ns));
    assert!(!(r.private_root || r.read_only_root || r.seccomp || r.capabilities_dropped));
    assert!(r.notes.iter().any(|n| n.contains("not isolation")), "{:?}", r.notes);
    assert!(o.environment_digest.starts_with("sha256:"));
}

#[test]
#[cfg(target_os = "linux")]
fn the_command_sees_the_host_exactly_as_the_calling_user_does() {
    // The honest part: nothing is hidden. A file outside any mount, the host's /proc, the network.
    let w = workdir();
    let secret_dir = workdir();
    let secret = secret_dir.path().join("secret.txt");
    std::fs::write(&secret, "TOP-SECRET").unwrap();
    assert_eq!(probe_line(&p(), w.path(), &["read", secret.to_str().unwrap()]), "ok 10");
    let me = std::process::id();
    assert_eq!(
        probe_line(&p(), w.path(), &["kill", &me.to_string()]),
        "ok 0",
        "the host process is visible and signalable"
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert_eq!(probe_line(&p(), w.path(), &["connect", &listener.local_addr().unwrap().to_string()]), "ok connected");
}

#[test]
fn the_environment_is_scrubbed_and_the_directory_is_the_work_tree() {
    let w = workdir();
    let o = run(&p(), &probe_spec_for(&p(), w.path(), &["env"]).env("ONLY", "this"));
    assert_eq!(o.stdout_lossy(), "ONLY=this\nPATH=/usr/bin:/bin\n");
    let o = run(&p(), &spec(&["pwd"], w.path()));
    assert_eq!(std::fs::canonicalize(o.stdout_lossy().trim()).unwrap(), std::fs::canonicalize(w.path()).unwrap());
}

#[test]
#[cfg(target_os = "linux")]
fn the_kernel_flag_that_blocks_privilege_escalation_is_set() {
    let w = workdir();
    assert_eq!(probe_status(&p(), w.path())["NoNewPrivs"], "1");
}

#[test]
#[cfg(target_os = "linux")]
fn the_timeout_kills_the_process_group_but_a_session_leader_can_escape() {
    let w = workdir();
    let (a, b, c) = (
        format!("1{}", unique_marker("x").len()),
        format!("2{}", std::process::id()),
        format!("3{}", std::process::id()),
    );
    // a child, a grandchild in the same group, and a process that starts its own session
    let script = format!("sleep {a} & (sleep {b} &) ; (setsid sleep {c} >/dev/null 2>&1 &) ; wait");
    let mut s = spec(&["sh", "-c", &script], w.path());
    s.limits.wall_ms = 500;
    let o = run(&p(), &s);
    assert_eq!(o.status, ExitStatus::TimedOut);
    std::thread::sleep(Duration::from_millis(100));
    assert!(host_processes_matching(&format!("sleep {a}")).is_empty(), "children in the process group are killed");
    assert!(host_processes_matching(&format!("sleep {b}")).is_empty());
    // The documented weakness: without a PID namespace a deliberate escapee survives. Clean it up.
    let escaped = host_processes_matching(&format!("sleep {c}"));
    assert_eq!(escaped.len(), 1, "the setsid escapee is expected to outlive the process sandbox: {escaped:?}");
    for (pid, _) in escaped {
        // SAFETY: killing the stray sleep process this test started.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

#[test]
fn output_stdin_and_exit_codes_behave_like_the_namespace_sandbox() {
    let w = workdir();
    let cap = 32 * 1024;
    let mut s = spec(&["yes"], w.path());
    s.limits.output_bytes = cap;
    let o = run(&p(), &s);
    assert_eq!((o.status, o.stdout.len() as u64, o.stdout_truncated), (ExitStatus::OutputLimit, cap, true));
    let o = run(&p(), &spec(&["cat"], w.path()).stdin(b"abc".to_vec()));
    assert_eq!(o.stdout_lossy(), "abc");
    for (script, want) in
        [("exit 7", ExitStatus::Exited(7)), ("kill -9 $$", ExitStatus::Signaled(9)), ("true", ExitStatus::Exited(0))]
    {
        assert_eq!(run(&p(), &spec(&["sh", "-c", script], w.path())).status, want, "{script}");
    }
    let err = p().run(&spec(&["no-such-program"], w.path())).unwrap_err();
    assert!(matches!(err, ag_sandbox::SandboxError::Exec { .. }), "{err}");
}

#[test]
#[cfg(target_os = "linux")]
fn resource_limits_apply_per_process() {
    let w = workdir();
    let mut s = probe_spec_for(&p(), w.path(), &["openfiles"]);
    s.limits.open_files = 30;
    let line = run(&p(), &s).stdout_lossy();
    assert!(line.starts_with("err EMFILE after "), "{line}");

    let mut s = probe_spec_for(&p(), w.path(), &["alloc", "4000", "1"]);
    s.limits = Limits { memory_bytes: 96 << 20, ..Limits::default() };
    let o = run(&p(), &s);
    let reached: u64 =
        o.stdout_lossy().lines().filter_map(|l| l.strip_prefix("allocated ")?.trim().parse().ok()).max().unwrap_or(0);
    assert!((32..=96).contains(&reached), "reached {reached} MiB against a 96 MiB RLIMIT_DATA\n{}", o.stderr_lossy());
    assert!(o.stdout_lossy().contains("alloc-failed"), "the allocation must fail, not succeed: {}", o.stdout_lossy());
    assert_eq!(o.isolation.memory, Enforcement::Rlimit);

    let mut s = probe_spec_for(&p(), w.path(), &["spin"]);
    s.limits = Limits { cpu_s: 1, wall_ms: 20_000, ..Limits::default() };
    assert_eq!(run(&p(), &s).status, ExitStatus::CpuLimit);
}

#[test]
#[cfg(target_os = "linux")]
fn the_task_limit_works_for_unprivileged_users_only() {
    let w = workdir();
    let mut s = probe_spec_for(&p(), w.path(), &["forkbomb", "100000"]);
    s.limits.pids = 25;
    if euid_egid().0 == 0 {
        // The kernel does not apply RLIMIT_NPROC to root, and the report must not pretend otherwise. (No
        // real fork bomb is run here: nothing would stop it.)
        let mut s = probe_spec_for(&p(), w.path(), &["status"]);
        s.limits.pids = 25;
        let o = run(&p(), &s);
        assert_eq!(o.isolation.tasks, Enforcement::None);
        assert!(o.isolation.host_root);
        assert!(o.isolation.notes.iter().any(|n| n.contains("exempts root")), "{:?}", o.isolation.notes);
        let _ = writeln!(
            std::io::stderr(),
            "NOTE the_task_limit_works_for_unprivileged_users_only: root host, fork bomb skipped"
        );
        return;
    }
    let o = run(&p(), &s);
    assert_eq!(o.isolation.tasks, Enforcement::Rlimit);
    let line = o.stdout_lossy();
    let forked: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    // The process sandbox's limit is "the user's tasks now + the allowance" (RLIMIT_NPROC counts the whole
    // user), so tasks the user starts or ends elsewhere meanwhile -- here: the other tests in this binary --
    // move it. The point is that the bomb is bounded, not the exact count.
    assert!(line.contains("stopped_by EAGAIN") && forked <= 25 + 200, "{line}");
}

#[test]
#[cfg(target_os = "linux")]
fn a_set_user_id_file_cannot_raise_privileges() {
    // Needs root to build the file. Under no_new_privs a root process stays root; without it, it would become
    // the file's owner. (The namespace sandbox's variant of this test is in isolation.rs.)
    if euid_egid().0 != 0 {
        let _ = writeln!(std::io::stderr(), "NOTE a_set_user_id_file_cannot_raise_privileges: needs root");
        return;
    }
    let dir = workdir();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let suid = dir.path().join("suid-probe");
    std::fs::copy(probe_path(), &suid).unwrap();
    std::os::unix::fs::chown(&suid, Some(1), Some(1)).unwrap();
    std::fs::set_permissions(&suid, std::fs::Permissions::from_mode(0o4755)).unwrap();
    let control = (0..200).find_map(|_| match std::process::Command::new(&suid).arg("euid").output() {
        Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
            std::thread::sleep(Duration::from_millis(10));
            None
        }
        other => Some(other.unwrap()),
    });
    if String::from_utf8_lossy(&control.expect("busy").stdout).trim() != "1" {
        let _ = writeln!(
            std::io::stderr(),
            "NOTE a_set_user_id_file_cannot_raise_privileges: this file system ignores set-user-ID"
        );
        return;
    }
    let w = workdir();
    let mut s = ag_sandbox::SandboxSpec::new([suid.to_str().unwrap(), "euid"], w.path());
    s.env.insert("PATH".into(), "/usr/bin:/bin".into());
    assert_eq!(run(&p(), &s).stdout_lossy().trim(), "0");
}
