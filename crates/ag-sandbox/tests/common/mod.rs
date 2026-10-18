//! Shared helpers for the integration tests.
//!
//! The tests run real programs inside real sandboxes. When the host cannot support the namespace sandbox
//! (no unprivileged user namespaces, a container runtime that forbids them, a non-Linux machine) each test
//! prints a `SKIP` line with the reason straight to the terminal's stderr — which `cargo test` does not
//! capture — and passes. Setting `AG_REQUIRE_SANDBOX=1` (CI does) turns every skip into a failure, so that
//! a runner that silently lost its isolation cannot go green.
#![allow(dead_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use ag_sandbox::{
    Availability, Mount, NamespaceSandbox, Outcome, ProcessSandbox, Sandbox, SandboxSpec, standard_system_mounts,
};

/// Is a skip forbidden? (`AG_REQUIRE_SANDBOX` set to anything but empty / `0` / `false`.)
pub fn skips_are_failures() -> bool {
    std::env::var("AG_REQUIRE_SANDBOX").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no"))
}

/// Report that a test cannot run here: fail under `AG_REQUIRE_SANDBOX`, otherwise print the reason.
pub fn skip(test: &str, reason: &str) {
    if skips_are_failures() {
        panic!("AG_REQUIRE_SANDBOX is set, but {test} cannot run on this host: {reason}");
    }
    // Written to the real stderr: the test harness would swallow `eprintln!` output of passing tests.
    let _ = writeln!(std::io::stderr(), "SKIP {test}: {reason}");
}

/// The namespace sandbox, or `None` (after reporting the skip) when this host cannot run it.
pub fn namespace(test: &str) -> Option<NamespaceSandbox> {
    let sb = NamespaceSandbox::new();
    match sb.availability() {
        Availability::Available => Some(sb),
        Availability::Unavailable(reason) => {
            skip(test, &format!("the namespace sandbox is unavailable: {reason}"));
            None
        }
    }
}

pub fn process() -> ProcessSandbox {
    ProcessSandbox::new()
}

/// Where the probe program lives on the host. `AG_SANDBOX_PROBE` overrides the path cargo compiled in
/// (used when the test binaries are copied somewhere an unprivileged user can reach).
pub fn probe_path() -> PathBuf {
    std::env::var_os("AG_SANDBOX_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_ag-sandbox-probe")))
}

pub const PROBE: &str = "/probe";

/// A spec that runs `argv` with the host's system directories and the probe program mounted read-only.
pub fn spec(argv: &[&str], workdir: &Path) -> SandboxSpec {
    SandboxSpec::new(argv.iter().copied(), workdir)
        .env("PATH", "/usr/bin:/bin")
        .ro_mounts(standard_system_mounts())
        .ro_mount(Mount::new(probe_path(), PROBE))
}

/// A spec running the probe with `args`. The probe is mounted at `/probe` for the namespace sandbox; the
/// process sandbox has no mounts and runs it from its host path (see [`probe_spec_for`]).
pub fn probe_spec(workdir: &Path, args: &[&str]) -> SandboxSpec {
    let mut argv = vec![PROBE];
    argv.extend_from_slice(args);
    spec(&argv, workdir)
}

/// Like [`probe_spec`], but with the probe's path as seen by `sb`.
pub fn probe_spec_for(sb: &dyn Sandbox, workdir: &Path, args: &[&str]) -> SandboxSpec {
    let mut s = probe_spec(workdir, args);
    if sb.isolation() == ag_sandbox::IsolationLevel::None {
        s.argv[0] = probe_path().to_string_lossy().to_string();
    }
    s
}

/// Run and insist that the sandbox itself worked.
pub fn run(sb: &dyn Sandbox, spec: &SandboxSpec) -> Outcome {
    match sb.run(spec) {
        Ok(o) => o,
        Err(e) => panic!("{} sandbox failed to run {:?}: {e}", sb.name(), spec.argv),
    }
}

/// Run the probe and return the first line of its stdout.
pub fn probe_line(sb: &dyn Sandbox, workdir: &Path, args: &[&str]) -> String {
    let o = run(sb, &probe_spec_for(sb, workdir, args));
    assert!(o.success(), "probe {args:?} ended with {:?}\nstderr: {}", o.status, o.stderr_lossy());
    o.stdout_lossy().lines().next().unwrap_or("").to_string()
}

/// Run `sh -c script` and return the outcome.
pub fn sh(sb: &dyn Sandbox, workdir: &Path, script: &str) -> Outcome {
    run(sb, &spec(&["sh", "-c", script], workdir))
}

/// A fresh working directory.
pub fn workdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

/// The `Key: value` lines of the probe's `status` command as a lookup.
pub fn probe_status(sb: &dyn Sandbox, workdir: &Path) -> std::collections::BTreeMap<String, String> {
    let o = run(sb, &probe_spec_for(sb, workdir, &["status"]));
    assert!(o.success(), "{:?}", o.status);
    o.stdout_lossy().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Host processes whose command line contains `marker` (the way to find a sandbox's leftovers from outside).
pub fn host_processes_matching(marker: &str) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    let me = std::process::id();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        if let Ok(cmd) = std::fs::read(entry.path().join("cmdline")) {
            let text = String::from_utf8_lossy(&cmd).replace('\0', " ");
            if text.contains(marker) {
                found.push((pid, text));
            }
        }
    }
    found
}

/// A string that appears in no other process's command line.
pub fn unique_marker(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}-{}-{}-{:x}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
    )
}

/// Children of this test process that are still there (running or zombie), by parent-pid.
pub fn children_of_this_process() -> Vec<(u32, String)> {
    let me = std::process::id();
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        // "pid (comm) S ppid ...": comm may contain spaces and parentheses; split after the last ')'
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { continue };
        let mut f = rest.split_whitespace();
        let state = f.next().unwrap_or("");
        let ppid: u32 = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if ppid == me {
            out.push((
                pid,
                format!(
                    "state {state}: {}",
                    String::from_utf8_lossy(&std::fs::read(entry.path().join("cmdline")).unwrap_or_default())
                        .replace('\0', " ")
                ),
            ));
        }
    }
    out
}

/// The effective user and group id of this process.
pub fn euid_egid() -> (u32, u32) {
    ag_sandbox::effective_ids()
}

/// The host name, as `hostname` prints it.
pub fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_string()
}

/// The address this host would use to reach the internet (a route lookup; nothing is sent), if it has one.
pub fn outbound_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}
