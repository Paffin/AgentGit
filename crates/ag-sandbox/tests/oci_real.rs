//! The OCI sandbox against a **real** container runtime (Docker or Podman), checked from *inside* the
//! container: the flags the sandbox passes are only worth something if the runtime accepts them and the
//! kernel enforces what they request.
//!
//! The tests need a runtime and an image that is already in its local store (the sandbox never pulls):
//!
//! * `AG_OCI_RUNTIME` — `docker`, `podman` or the path of a compatible executable (default: the first of
//!   `docker`, `podman` found on `PATH`);
//! * `AG_OCI_IMAGE` — a digest-pinned reference to a BusyBox image (`busybox@sha256:...`); the CI workflow
//!   (`.github/workflows/oci-runtimes.yml`) pulls it before the tests run;
//! * `AG_REQUIRE_OCI=1` — a host that cannot run them fails instead of skipping. CI sets it.
//!
//! Without a usable runtime and image each test prints `SKIP <name>: <reason>` and passes, so a laptop
//! without Docker stays green; it also does not count as having verified anything.
//!
//! Every test holds one lock: the leftovers check looks at all containers of this sandbox, and the memory
//! and CPU tests should not compete with each other for the machine.
#![cfg(target_os = "linux")]

mod common;

use std::io::Write as _;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ag_sandbox::{
    Enforcement, Engine, ExitStatus, Mount, Network, OciConfig, OciSandbox, Outcome, RuntimeInfo, Sandbox,
    SandboxError, SandboxSpec,
};

static LOCK: Mutex<()> = Mutex::new(());

/// A runtime and an image that work here.
struct Real {
    runtime: String,
    image: String,
    info: RuntimeInfo,
    user: String,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn require() -> bool {
    std::env::var("AG_REQUIRE_OCI").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no"))
}

fn skip(test: &str, reason: &str) -> Option<Real> {
    if require() {
        panic!("AG_REQUIRE_OCI is set, but {test} cannot run on this host: {reason}");
    }
    let _ = writeln!(std::io::stderr(), "SKIP {test}: {reason}");
    None
}

fn real(test: &str) -> Option<Real> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = match std::env::var("AG_OCI_RUNTIME") {
        Ok(r) if !r.trim().is_empty() => r,
        _ => {
            let found = ["docker", "podman"].into_iter().find(|rt| {
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| d.join(rt).is_file())
            });
            match found {
                Some(rt) => rt.to_string(),
                None => return skip(test, "neither docker nor podman is on PATH (set AG_OCI_RUNTIME)"),
            }
        }
    };
    let Ok(image) = std::env::var("AG_OCI_IMAGE") else {
        return skip(test, "AG_OCI_IMAGE (a digest-pinned BusyBox image in the local store) is not set");
    };
    let probe = OciSandbox::new(&runtime, &image).expect("a valid runtime and image reference");
    let info = match probe.probe() {
        Ok(i) => i,
        Err(e) => return skip(test, &format!("the runtime is not usable: {e}")),
    };
    if let Err(e) = probe.image_present() {
        return skip(test, &format!("the image is not available: {e}"));
    }
    let (euid, egid) = ag_sandbox::effective_ids();
    let user = info.user_for_tree(euid, egid, "65534:65534");
    Some(Real { runtime, image, info, user, _guard: guard })
}

impl Real {
    fn sandbox(&self) -> OciSandbox {
        let mut cfg = OciConfig::new(&self.runtime, &self.image);
        cfg.user = self.user.clone();
        OciSandbox::with_config(cfg).expect("a valid configuration")
    }

    /// A working tree the container user can write: owned by it, which is what the worker does when it runs
    /// as root (a non-root caller already owns the tree, and the user was chosen to match).
    fn workdir(&self) -> tempfile::TempDir {
        let w = common::workdir();
        self.share(w.path());
        w
    }

    /// Hand a directory to the container user (only possible, and only needed, for a root caller).
    fn share(&self, dir: &Path) {
        let (euid, _) = ag_sandbox::effective_ids();
        if euid == 0 && self.info.rootless != Some(true) {
            let (u, g) = self.user.split_once(':').unwrap();
            std::os::unix::fs::chown(dir, u.parse().ok(), g.parse().ok())
                .expect("chown the tree to the container user");
        }
    }

    fn spec(&self, script: &str, workdir: &Path) -> SandboxSpec {
        SandboxSpec::new(["sh", "-c", script], workdir).env("PATH", "/bin:/usr/bin").env("CHECK_VAR", "visible")
    }

    /// Run `script` with `sh -c` and insist that the sandbox itself worked.
    fn sh(&self, script: &str) -> Outcome {
        let w = self.workdir();
        self.sh_in(script, w.path(), |_| {})
    }

    fn sh_in(&self, script: &str, w: &Path, edit: impl FnOnce(&mut SandboxSpec)) -> Outcome {
        let mut spec = self.spec(script, w);
        edit(&mut spec);
        match self.sandbox().run(&spec) {
            Ok(o) => o,
            Err(e) => panic!("the OCI sandbox failed to run {script:?}: {e}"),
        }
    }

    /// Run a runtime command on the host and return its stdout.
    fn host(&self, args: &[&str]) -> String {
        let out = Command::new(&self.runtime).args(args).output().expect("the runtime runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Containers of this sandbox that still exist (running or not), each with what the runtime says about it.
    fn leftovers(&self) -> Vec<String> {
        let prefix = format!("name=ag-sandbox-{}-", std::process::id());
        let names =
            self.host(&["ps", "-a", "--filter", "label=ag-sandbox=1", "--filter", &prefix, "--format", "{{.Names}}"]);
        names
            .split_whitespace()
            .map(|name| {
                let state = self.host(&[
                    "inspect",
                    "--format",
                    "{{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}} cmd={{.Path}} {{.Args}}",
                    name,
                ]);
                format!("{name} [{}]", state.trim())
            })
            .collect()
    }
}

/// The value of a `Key:<whitespace>value` line (as in `/proc/self/status`).
fn field(out: &str, key: &str) -> Option<String> {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}:"))).map(|v| v.trim().to_string())
}

/// The value of a `key=value` line.
fn kv(out: &str, key: &str) -> Option<String> {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).map(|v| v.trim().to_string())
}

/// Shell: print the first of these cgroup files that can be read (cgroup v2 first, then v1).
const CGROUP_FILE: &str =
    r#"cg() { for f in "$@"; do if [ -r "$f" ]; then cat "$f"; return; fi; done; echo unreadable; }"#;

#[test]
fn the_kernels_own_view_inside_the_container_matches_what_was_requested() {
    let Some(r) = real("the_kernels_own_view_inside_the_container_matches_what_was_requested") else { return };
    let w = r.workdir();
    let o = r.sh_in(
        r#"grep -E '^(NoNewPrivs|Seccomp|CapEff|CapBnd|CapPrm|CapInh|CapAmb):' /proc/self/status
echo "uid=$(id -u) gid=$(id -g)"
echo "hostname=$(hostname)"
echo "processes=$(ls /proc | grep -c '^[0-9][0-9]*$')"
echo "pid1=$(tr '\0' ' ' < /proc/1/cmdline)"
{ touch /rootfile; } 2>&1 | sed 's/^/root_write: /'
{ touch /etc/rootfile; } 2>&1 | sed 's/^/etc_write: /'
echo "work_write=$(echo proof > /work/proof && echo ok)"
echo "ifaces=$(tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\n' ' ')"
echo "docker_sock=$(ls /var/run/docker.sock 2>&1 | head -1)"
"#,
        w.path(),
        |_| {},
    );
    let out = o.stdout_lossy();
    assert_eq!(o.status, ExitStatus::Exited(0), "{out}\n{}", o.stderr_lossy());
    assert_eq!(field(&out, "NoNewPrivs").as_deref(), Some("1"), "no new privileges\n{out}");
    assert_eq!(field(&out, "Seccomp").as_deref(), Some("2"), "a seccomp filter is installed\n{out}");
    for cap in ["CapEff", "CapBnd", "CapPrm", "CapInh", "CapAmb"] {
        assert_eq!(field(&out, cap).as_deref(), Some("0000000000000000"), "{cap}: no capabilities\n{out}");
    }
    let (user, group) = r.user.split_once(':').unwrap();
    assert!(out.contains(&format!("uid={user} gid={group}")), "the configured user\n{out}");
    assert!(out.contains("root_write: touch: /rootfile: Read-only file system"), "read-only root\n{out}");
    assert!(out.contains("etc_write: touch: /etc/rootfile: Read-only file system"), "read-only /etc\n{out}");
    assert_eq!(kv(&out, "work_write").as_deref(), Some("ok"), "the working tree is writable\n{out}");
    assert_eq!(kv(&out, "ifaces").as_deref(), Some("lo"), "only the loopback interface exists\n{out}");
    assert!(
        out.contains("docker_sock=ls: /var/run/docker.sock: No such file or directory"),
        "no runtime socket\n{out}"
    );
    assert!(out.contains("pid1=sh -c"), "the command is the container's init, nothing of the host is\n{out}");
    let visible: usize = kv(&out, "processes").unwrap().parse().unwrap();
    assert!(visible < 20, "only the container's own processes are visible, not the host's ({visible})\n{out}");
    assert_ne!(kv(&out, "hostname").unwrap_or_default(), common::host_name(), "a private UTS namespace\n{out}");

    // what the command wrote shows up on the host, owned by the user it ran as
    assert_eq!(std::fs::read_to_string(w.path().join("proof")).unwrap(), "proof\n");
    use std::os::unix::fs::MetadataExt;
    let owner = std::fs::metadata(w.path().join("proof")).unwrap().uid();
    let (euid, _) = ag_sandbox::effective_ids();
    if r.info.rootless == Some(true) {
        assert_eq!(owner, euid, "rootless: container root is the host user");
    } else {
        assert_eq!(owner.to_string(), user, "rootful: the same id on both sides");
    }
}

#[test]
fn the_report_is_what_the_runtime_recorded_and_stable() {
    let Some(r) = real("the_report_is_what_the_runtime_recorded_and_stable") else { return };
    let a = r.sh("true");
    let i = &a.isolation;
    assert!(a.success(), "{:?}\n{}", a.status, a.stderr_lossy());
    assert!(i.verified, "every protection was read back from the runtime's record: {:?}", i.notes);
    assert!(i.net_ns && i.read_only_root && i.no_new_privs && i.capabilities_dropped && i.rlimits, "{i:?}");
    assert!(i.private_root && i.mount_ns && i.pid_ns && i.ipc_ns && i.uts_ns, "{i:?}");
    assert!(i.seccomp, "the daemon applies a seccomp profile by default: {:?}", r.info);
    assert_eq!(i.user_ns, r.info.user_ns == Some(true));
    assert!(!i.host_root, "the container user is not host root");
    assert_eq!((i.memory, i.tasks), (Enforcement::Runtime, Enforcement::Runtime));
    assert_eq!(i.cgroup.map(|c| (c.memory, c.pids)), Some((true, true)), "{:?}", i.notes);
    // the same run twice is the same environment; another limit is another environment
    let b = r.sh("true");
    assert_eq!(a.environment_digest, b.environment_digest);
    let w = r.workdir();
    let c = r.sh_in("true", w.path(), |s| s.limits.memory_bytes = 96 << 20);
    assert_ne!(a.environment_digest, c.environment_digest);
    assert!(r.leftovers().is_empty(), "{:?}", r.leftovers());
}

#[test]
fn there_is_no_network_unless_the_host_network_was_asked_for() {
    let Some(r) = real("there_is_no_network_unless_the_host_network_was_asked_for") else { return };
    // a listener on every host address: unreachable from the container, reachable with --network host
    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let host_ip = common::outbound_ip().unwrap_or_else(|| "127.0.0.1".into());
    let probe = format!(
        "for a in 127.0.0.1 {host_ip}; do if nc -w 2 $a {port} </dev/null >/dev/null 2>&1; then echo \"reached $a\"; else echo \"no route to $a\"; fi; done"
    );
    let isolated = r.sh(&probe);
    let out = isolated.stdout_lossy();
    assert!(out.contains("no route to 127.0.0.1") && out.contains(&format!("no route to {host_ip}")), "{out}");
    assert!(!out.contains("reached"), "{out}");
    assert!(listener.accept().is_err(), "nothing connected to the host's listener");
    assert!(isolated.isolation.net_ns);

    // positive control: the same probe with the host network does reach the listener, so the test can fail
    let w = r.workdir();
    let shared = r.sh_in(&probe, w.path(), |s| s.network = Network::Host);
    let out = shared.stdout_lossy();
    assert!(out.contains("reached 127.0.0.1"), "the host network reaches the host's loopback listener\n{out}");
    assert!(!shared.isolation.net_ns, "and the report says the network is not isolated");
    assert_ne!(isolated.environment_digest, shared.environment_digest);
}

#[test]
fn the_memory_limit_is_set_in_the_cgroup_and_a_hog_is_killed_by_the_kernel() {
    let Some(r) = real("the_memory_limit_is_set_in_the_cgroup_and_a_hog_is_killed_by_the_kernel") else { return };
    let limit: u64 = 64 << 20;
    // what the container's own cgroup says
    let w = r.workdir();
    let o = r.sh_in(
        &format!("{CGROUP_FILE}\ncg /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory/memory.limit_in_bytes"),
        w.path(),
        |s| s.limits.memory_bytes = limit,
    );
    assert_eq!(o.stdout_lossy().trim(), limit.to_string(), "memory.max inside the container\n{}", o.stderr_lossy());
    // a program under the limit is fine
    let fine = r.sh_in("head -c 16000000 /dev/zero | tr '\\0' x | wc -c", w.path(), |s| s.limits.memory_bytes = limit);
    assert_eq!(fine.status, ExitStatus::Exited(0), "{}", fine.stderr_lossy());
    // One that doubles a string until it is killed. It is the container's own process (`exec`): the runtime
    // learns of an OOM kill from the cgroup's event, and Docker's record of it is only reliable when the killed
    // process is the one whose exit ends the container (a shell that outlives its child and exits 137 itself
    // races the event, as the CI run of 27bbce1 showed). 137 is SIGKILL, the kernel's OOM kill; the program's
    // own allocation failure would end with another status.
    let hog = r.sh_in("exec awk 'BEGIN { s = \"xxxxxxxxxxxxxxxx\"; while (1) s = s s }'", w.path(), |s| {
        s.limits.memory_bytes = limit
    });
    assert!(
        matches!(hog.status, ExitStatus::MemoryLimit | ExitStatus::Exited(137)),
        "the kernel killed the hog: {:?} {}",
        hog.status,
        hog.stderr_lossy()
    );
    let noted = hog.isolation.notes.iter().any(|n| n.contains("does not record one in `inspect`"));
    match (r.info.engine, hog.status) {
        // Docker records the kernel's OOM kill and the sandbox says so.
        (Engine::Docker, status) => assert_eq!(status, ExitStatus::MemoryLimit, "{}", hog.stderr_lossy()),
        // Podman (4.9, in CI) does not set `State.OOMKilled` for the same kill, so there is nothing in its record
        // to call it a memory kill by: the sandbox reports the exit status it has and says why in the notes.
        (Engine::Podman, ExitStatus::Exited(137)) => assert!(noted, "{:?}", hog.isolation.notes),
        (Engine::Podman, ExitStatus::MemoryLimit) => assert!(!noted, "a runtime that records the kill needs no excuse"),
        (_, other) => panic!("{other:?}: {}", hog.stderr_lossy()),
    }
    assert!(r.leftovers().is_empty());
}

#[test]
fn the_pids_limit_stops_a_fork_bomb_at_the_limit() {
    let Some(r) = real("the_pids_limit_stops_a_fork_bomb_at_the_limit") else { return };
    let w = r.workdir();
    // The bomb runs in a child shell (a shell that cannot fork gives up, as busybox's does); this shell then
    // counts what is left without forking: the entries of /proc, and the cgroup's own counters.
    let o = r.sh_in(
        &format!(
            r#"{CGROUP_FILE}
echo "pids_max=$(cg /sys/fs/cgroup/pids.max /sys/fs/cgroup/pids/pids.max)"
sh -c 'i=0; while [ $i -lt 100 ]; do sleep 30 & i=$((i+1)); done' 2>/dev/null
n=0; for p in /proc/[0-9]*; do n=$((n+1)); done
echo "tasks=$n"
for f in /sys/fs/cgroup/pids.current /sys/fs/cgroup/pids/pids.current; do [ -r $f ] && {{ read c < $f; echo "current=$c"; }}; done
for f in /sys/fs/cgroup/pids.events /sys/fs/cgroup/pids/pids.events; do [ -r $f ] && while read k v; do echo "event_$k=$v"; done < $f; done
true"#
        ),
        w.path(),
        |s| s.limits.pids = 16,
    );
    let out = o.stdout_lossy();
    assert_eq!(
        kv(&out, "pids_max").as_deref(),
        Some("16"),
        "pids.max inside the container\n{out}\n{}",
        o.stderr_lossy()
    );
    let tasks: u64 =
        kv(&out, "tasks").and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("{out}\n{}", o.stderr_lossy()));
    assert!(tasks > 8, "the bomb did fork ({tasks} tasks)\n{out}");
    assert!(tasks <= 16, "at most 16 tasks exist in the container ({tasks})\n{out}");
    if let Some(current) = kv(&out, "current").and_then(|v| v.parse::<u64>().ok()) {
        assert!(current <= 16, "the cgroup counted {current}\n{out}");
    }
    if let Some(denied) = kv(&out, "event_max").and_then(|v| v.parse::<u64>().ok()) {
        assert!(denied > 0, "the kernel refused forks beyond the limit\n{out}");
    }
    assert!(r.leftovers().is_empty());
}

#[test]
fn the_cpu_quota_and_the_cpu_time_limit_are_enforced() {
    let Some(r) = real("the_cpu_quota_and_the_cpu_time_limit_are_enforced") else { return };
    // --cpus 1: the cgroup carries a quota of one CPU
    let quota = r.sh(&format!(
        "{CGROUP_FILE}\ncg /sys/fs/cgroup/cpu.max /sys/fs/cgroup/cpu/cpu.cfs_quota_us /sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us"
    ));
    let quota = quota.stdout_lossy();
    assert!(
        quota.trim() == "100000 100000" || quota.trim() == "100000",
        "a quota of one CPU (cpu.max / cfs_quota_us), got {quota:?}"
    );
    // ... and three busy loops over two seconds do not use more than about one CPU
    let busy = r.sh(&format!(
        r#"{CGROUP_FILE}
for i in 1 2 3; do sh -c 'while :; do :; done' & done
sleep 2
u=$(cg /sys/fs/cgroup/cpu.stat /sys/fs/cgroup/cpuacct/cpuacct.usage /sys/fs/cgroup/cpu,cpuacct/cpuacct.usage)
echo "$u" | tr '\n' ' '
kill $(jobs -p)"#
    ));
    let text = busy.stdout_lossy();
    let cpu_seconds = if let Some(rest) = text.split("usage_usec ").nth(1) {
        rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).map(|us| us / 1e6)
    } else {
        text.trim().parse::<f64>().ok().map(|ns| ns / 1e9)
    };
    let used = cpu_seconds.unwrap_or_else(|| panic!("could not read the CPU usage: {text:?}\n{}", busy.stderr_lossy()));
    assert!(used < 2.9, "three busy loops over 2 s used {used:.2} CPU-seconds; a 1-CPU quota allows about 2\n{text}");
    assert!(used > 0.5, "the loops did run ({used:.2})");

    // --ulimit cpu, a child of the container's init: the kernel sends SIGXCPU, which is recognised as the limit
    let w = r.workdir();
    let started = Instant::now();
    let child = r.sh_in("sh -c 'while :; do :; done'; echo \"child status $?\"", w.path(), |s| {
        s.limits.cpu_s = 1;
        s.limits.wall_ms = 30_000;
    });
    assert!(
        child.stdout_lossy().contains("child status 152"),
        "the busy loop died of SIGXCPU: {:?}",
        child.stdout_lossy()
    );
    assert!(started.elapsed() < Duration::from_secs(20));

    // ... and the container's own init (PID 1) is exempt from SIGXCPU: the kernel kills it with SIGKILL at the
    // hard limit instead (cpu_s + 1), which no one can tell from any other kill. Still enforced; not labelled.
    let started = Instant::now();
    let init = r.sh_in("while :; do :; done", w.path(), |s| {
        s.limits.cpu_s = 1;
        s.limits.wall_ms = 30_000;
    });
    assert_eq!(init.status, ExitStatus::Exited(137), "{}", init.stderr_lossy());
    assert!(started.elapsed() < Duration::from_secs(20), "killed by the CPU limit, not by the wall clock");
    assert!(r.leftovers().is_empty());
}

#[test]
fn file_descriptor_core_and_file_size_limits_are_in_force() {
    let Some(r) = real("file_descriptor_core_and_file_size_limits_are_in_force") else { return };
    let w = r.workdir();
    let o = r.sh_in(
        "echo \"nofile=$(ulimit -Hn) core=$(ulimit -Hc)\"; dd if=/dev/zero of=/work/big bs=1024 count=4096 2>&1; echo \"status=$?\"",
        w.path(),
        |s| {
            s.limits.open_files = 128;
            s.limits.file_size_bytes = 1 << 20;
        },
    );
    let out = o.stdout_lossy();
    assert!(out.contains("nofile=128 core=0"), "{out}");
    let size = std::fs::metadata(w.path().join("big")).map(|m| m.len()).unwrap_or(0);
    assert!(size <= 1 << 20, "RLIMIT_FSIZE stopped the write at {size} bytes\n{out}");
    assert!(size > 0, "the write started\n{out}");
    assert!(!out.contains("status=0"), "and was cut short\n{out}");
}

#[test]
fn tmp_is_a_size_limited_tmpfs_and_read_only_mounts_stay_read_only() {
    let Some(r) = real("tmp_is_a_size_limited_tmpfs_and_read_only_mounts_stay_read_only") else { return };
    let ro = common::workdir();
    std::fs::set_permissions(ro.path(), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(ro.path().join("tool"), "a toolchain\n").unwrap();
    let w = r.workdir();
    let o = r.sh_in(
        r#"echo "tmp_kib=$(df -k /tmp | tail -1 | tr -s ' ' | cut -d' ' -f2)"
echo hi > /tmp/small && echo tmp_small=ok
echo "tmp_big_full=$({ dd if=/dev/zero of=/tmp/big bs=1024 count=4096; } 2>&1 | grep -c 'No space left on device')"
cat /opt/tools/tool
{ echo x > /opt/tools/tool; } 2>&1 | sed 's/^/ro_write: /'
{ touch /opt/tools/new; } 2>&1 | sed 's/^/ro_create: /'"#,
        w.path(),
        |s| {
            s.rw_tmp_bytes = 2 << 20;
            s.ro_mounts.push(Mount::new(ro.path(), "/opt/tools"));
        },
    );
    let out = o.stdout_lossy();
    assert_eq!(kv(&out, "tmp_kib").as_deref(), Some("2048"), "/tmp is 2 MiB\n{out}\n{}", o.stderr_lossy());
    assert!(out.contains("tmp_small=ok"), "{out}");
    assert_eq!(
        kv(&out, "tmp_big_full").as_deref(),
        Some("1"),
        "writing past the size of /tmp fails with ENOSPC\n{out}"
    );
    assert!(out.contains("a toolchain"), "{out}");
    assert!(out.contains("ro_write: sh: can't create /opt/tools/tool: Read-only file system"), "{out}");
    assert!(out.contains("ro_create: touch: /opt/tools/new: Read-only file system"), "{out}");
    assert_eq!(std::fs::read_to_string(ro.path().join("tool")).unwrap(), "a toolchain\n", "the host file is untouched");
    assert!(o.isolation.verified, "{:?}", o.isolation.notes);
}

#[test]
fn nothing_of_the_workers_keys_or_environment_reaches_the_container() {
    let Some(r) = real("nothing_of_the_workers_keys_or_environment_reaches_the_container") else { return };
    // The "worker": a signing key file next to the working tree, with a secret in it. The test process's own
    // environment (cargo sets CARGO_MANIFEST_DIR and friends) is what the runtime client is started from.
    let state = common::workdir();
    let secret = format!("secret{}x{:x}", std::process::id(), std::time::UNIX_EPOCH.elapsed().unwrap().subsec_nanos());
    std::fs::write(state.path().join("worker-key.json"), format!("{{\"secret\":\"{secret}\"}}")).unwrap();
    let w = r.workdir();
    let split = format!("{}\"\"{}", &secret[..5], &secret[5..]);
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let script = format!(
        r#"needle="{split}"
echo "env_has_cargo=$(env | grep -c CARGO_)"
echo "env_has_manifest_dir=$(env | grep -c 'ag-sandbox$')"
echo "proc_environ=$(cat /proc/[0-9]*/environ 2>/dev/null | tr '\0' '\n' | grep -c "$needle")"
echo "files=$(grep -rl "$needle" /bin /etc /home /lib /root /run /sbin /tmp /usr /var /work /mnt /opt /srv 2>/dev/null | wc -l)"
echo "key_file=$(find / -xdev -name 'worker-key*' 2>/dev/null | wc -l)"
echo "host_state=$(ls {state} 2>&1 | head -1)"
echo "host_source=$(ls {manifest_dir} 2>&1 | head -1)"
echo "env_file_visible=$(ls /tmp/ag-sandbox-env* 2>/dev/null | wc -l)"
echo "check_var=$CHECK_VAR"
"#,
        state = state.path().display()
    );
    let o = r.sh_in(&script, w.path(), |_| {});
    let out = o.stdout_lossy();
    assert_eq!(o.status, ExitStatus::Exited(0), "{out}\n{}", o.stderr_lossy());
    assert_eq!(kv(&out, "env_has_cargo").as_deref(), Some("0"), "the worker's environment is not inherited\n{out}");
    assert_eq!(kv(&out, "env_has_manifest_dir").as_deref(), Some("0"), "{out}");
    assert_eq!(kv(&out, "proc_environ").as_deref(), Some("0"), "{out}");
    assert_eq!(kv(&out, "files").as_deref(), Some("0"), "the secret is in no file the container can read\n{out}");
    assert_eq!(
        kv(&out, "key_file").as_deref(),
        Some("0"),
        "no key file anywhere in the container's file system\n{out}"
    );
    assert!(out.contains("host_state=ls: ") && out.contains("No such file or directory"), "{out}");
    assert!(kv(&out, "host_source").unwrap().starts_with("ls: "), "the host's source tree is not there\n{out}");
    assert_eq!(kv(&out, "env_file_visible").as_deref(), Some("0"), "the environment file is not mounted\n{out}");
    assert_eq!(kv(&out, "check_var").as_deref(), Some("visible"), "only the spec's own environment arrives\n{out}");
    // the environment file that carried the spec's variables is gone from the host
    let prefix = format!("ag-sandbox-env-{}-", std::process::id());
    let left = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .count();
    assert_eq!(left, 0, "the environment file is removed");
}

#[test]
fn exit_statuses_streams_and_stdin_come_through_a_real_runtime() {
    let Some(r) = real("exit_statuses_streams_and_stdin_come_through_a_real_runtime") else { return };
    let ran = |script: &str| {
        let w = r.workdir();
        r.sh_in(script, w.path(), |_| {})
    };
    assert_eq!(ran("exit 3").status, ExitStatus::Exited(3));
    // exit codes that look like the runtime's own are the command's when the command ran (finding 40)
    let o = ran("echo 'Error: it is the check speaking' >&2; exit 125");
    assert_eq!(o.status, ExitStatus::Exited(125), "{}", o.stderr_lossy());
    assert!(o.stderr_lossy().contains("it is the check speaking"));
    assert_eq!(ran("exit 126").status, ExitStatus::Exited(126));
    assert_eq!(ran("exit 127").status, ExitStatus::Exited(127));
    let o = ran("echo out; echo err >&2");
    assert_eq!((o.stdout_lossy().as_str(), o.stderr_lossy().as_str()), ("out\n", "err\n"));

    let w = r.workdir();
    let o = r.sh_in("cat", w.path(), |s| s.stdin = b"through the pipe\n".to_vec());
    assert_eq!(o.stdout_lossy(), "through the pipe\n");

    // a command the image does not have never ran: that is not an exit status
    let mut spec = r.spec("true", w.path());
    spec.argv = vec!["definitely-not-in-the-image".into()];
    match r.sandbox().run(&spec) {
        Err(SandboxError::Exec { program, source }) => {
            assert_eq!(program, "definitely-not-in-the-image");
            assert!(!source.to_string().is_empty());
        }
        other => panic!("a missing command is an Exec error, got {other:?}"),
    }
    assert!(r.leftovers().is_empty(), "{:?}", r.leftovers());
}

#[test]
fn a_timeout_or_an_output_flood_kills_the_container_and_nothing_is_left() {
    let Some(r) = real("a_timeout_or_an_output_flood_kills_the_container_and_nothing_is_left") else { return };
    let marker = 600 + (std::process::id() % 300);
    let w = r.workdir();
    let started = Instant::now();
    let o = r.sh_in(&format!("sleep {marker}"), w.path(), |s| s.limits.wall_ms = 1500);
    assert_eq!(o.status, ExitStatus::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    assert!(r.leftovers().is_empty(), "the container was removed: {:?}", r.leftovers());
    assert!(
        common::host_processes_matching(&format!("sleep {marker}")).is_empty(),
        "the command's process is gone from the host"
    );

    let flood = r.sh_in(
        "while :; do echo yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy; done",
        w.path(),
        |s| s.limits.output_bytes = 4096,
    );
    assert_eq!(flood.status, ExitStatus::OutputLimit);
    assert_eq!(flood.stdout.len(), 4096);
    assert!(r.leftovers().is_empty(), "{:?}", r.leftovers());
}

#[test]
fn cancelling_a_run_kills_and_removes_the_container() {
    let Some(r) = real("cancelling_a_run_kills_and_removes_the_container") else { return };
    let marker = 900 + (std::process::id() % 300);
    let w = r.workdir();
    let spec = r.spec(&format!("sleep {marker}"), w.path());
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let sandbox = r.sandbox();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            stop.store(true, Ordering::SeqCst);
        });
        sandbox.run_cancellable(&spec, &|| stop.load(Ordering::SeqCst))
    });
    assert!(matches!(result, Err(SandboxError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    assert!(r.leftovers().is_empty(), "{:?}", r.leftovers());
    assert!(common::host_processes_matching(&format!("sleep {marker}")).is_empty());
}

#[test]
fn a_missing_image_is_a_runtime_error_found_before_any_work_and_the_sandbox_never_pulls() {
    let Some(r) = real("a_missing_image_is_a_runtime_error_found_before_any_work_and_the_sandbox_never_pulls") else {
        return;
    };
    let absent = format!("{}@sha256:{}", r.image.split('@').next().unwrap(), "0123456789abcdef".repeat(4));
    let mut cfg = OciConfig::new(&r.runtime, &absent);
    cfg.user = r.user.clone();
    let sb = OciSandbox::with_config(cfg).unwrap();
    let err = sb.preflight().unwrap_err();
    assert!(matches!(&err, SandboxError::Runtime(m) if m.contains("never pulls")), "{err}");
    let w = r.workdir();
    let err = sb.run(&r.spec("true", w.path())).unwrap_err();
    assert!(matches!(err, SandboxError::Runtime(_)), "{err}");
    assert!(r.leftovers().is_empty(), "{:?}", r.leftovers());
}

#[test]
fn orphans_of_a_dead_worker_are_found_by_their_label_and_removed() {
    let Some(r) = real("orphans_of_a_dead_worker_are_found_by_their_label_and_removed") else { return };
    let host = common::host_name();
    let dead = "ag-sandbox-1073741825-0-orphan";
    let alive = format!("ag-sandbox-{}-0-mine", std::process::id());
    for name in [dead, alive.as_str()] {
        let out = Command::new(&r.runtime)
            .args(["create", "--name", name, "--label", "ag-sandbox=1", "--label"])
            .arg(format!("ag-sandbox.host={host}"))
            .args([&r.image, "true"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let removed = r.sandbox().reap_orphans().unwrap();
    assert_eq!(removed, [dead], "only the container of the dead process is removed");
    let left = r.host(&["ps", "-a", "--filter", "label=ag-sandbox=1", "--format", "{{.Names}}"]);
    assert!(left.contains(&alive) && !left.contains(dead), "{left}");
    let _ = Command::new(&r.runtime).args(["rm", "-f", &alive]).output();
}
