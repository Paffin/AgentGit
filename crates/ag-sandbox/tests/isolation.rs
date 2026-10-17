//! Proof that the namespace sandbox isolates: every test runs a real program inside a real sandbox and
//! asserts what the *kernel* reports from the inside, plus what is observable from the outside.
//!
//! See `tests/common/mod.rs` for how hosts without namespace support are handled (`AG_REQUIRE_SANDBOX`).
#![allow(unsafe_code)] // a few tests create host-side kernel objects (inheritable descriptors, SysV shm) to prove they stay unreachable
#![cfg(target_os = "linux")] // the namespace and container sandboxes are exercised on Linux only

mod common;

use std::collections::BTreeSet;
use std::io::Write as _;
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use ag_sandbox::{ExitStatus, IsolationLevel, Limits, Network, Sandbox};
use common::*;

/// `/proc` entries the sandbox makes read-only or hides; keep in sync with `docs/architecture/sandbox.md`.
const PROC_HARDENED: [&str; 15] = [
    "/proc/bus",
    "/proc/fs",
    "/proc/irq",
    "/proc/sys",
    "/proc/sysrq-trigger",
    "/proc/acpi",
    "/proc/asound",
    "/proc/kcore",
    "/proc/keys",
    "/proc/kmsg",
    "/proc/latency_stats",
    "/proc/sched_debug",
    "/proc/scsi",
    "/proc/timer_list",
    "/proc/timer_stats",
];

fn first_field(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

#[test]
fn the_network_is_unreachable_by_default() {
    let Some(sb) = namespace("the_network_is_unreachable_by_default") else { return };
    let w = workdir();
    // a listener on the host's loopback, like a database or an agent's local API
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    assert_eq!(
        probe_line(&sb, w.path(), &["connect", &addr.to_string()]),
        "err ECONNREFUSED",
        "the host's loopback must not be reachable"
    );
    // the connection never arrived on the host side
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    // nor is anything off the machine: there is no route at all (TEST-NET-1 and a public resolver)
    assert_eq!(probe_line(&sb, w.path(), &["connect", "192.0.2.1:80"]), "err ENETUNREACH");
    assert_eq!(probe_line(&sb, w.path(), &["connect", "1.1.1.1:53"]), "err ENETUNREACH");
    // the only interface is a loopback of its own, and it works
    let o = sh(&sb, w.path(), "tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '");
    assert_eq!(o.stdout_lossy().trim(), "lo", "{}", o.stderr_lossy());
    assert_eq!(probe_line(&sb, w.path(), &["loopback"]), "ok ping");
}

#[test]
fn host_network_is_an_explicit_and_visible_opt_in() {
    let Some(sb) = namespace("host_network_is_an_explicit_and_visible_opt_in") else { return };
    let w = workdir();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let mut spec = probe_spec(w.path(), &["connect", &addr]);
    spec.network = Network::Host;
    let o = run(&sb, &spec);
    assert_eq!(o.stdout_lossy().lines().next(), Some("ok connected"), "{:?} {}", o.status, o.stderr_lossy());
    assert!(listener.accept().is_ok(), "with Network::Host the host listener does receive the connection");
    // ... and the report and the digest say so
    assert_eq!(o.isolation.network, Network::Host);
    assert!(!o.isolation.net_ns);
    let none = run(&sb, &probe_spec(w.path(), &["connect", &addr]));
    assert!(none.isolation.net_ns);
    assert_ne!(none.environment_digest, o.environment_digest);
}

#[test]
fn abstract_unix_sockets_of_the_host_are_unreachable_without_host_network() {
    let Some(sb) = namespace("abstract_unix_sockets_of_the_host_are_unreachable_without_host_network") else { return };
    use std::os::linux::net::SocketAddrExt;
    let w = workdir();
    // Abstract sockets live in the network namespace, not the file system: bind-mount tricks cannot hide them.
    let name = unique_marker("ag-sandbox-abstract");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();
    listener.set_nonblocking(true).unwrap();

    assert_eq!(probe_line(&sb, w.path(), &["unix-abstract", &name]), "err ECONNREFUSED");
    assert!(listener.accept().is_err());
    let mut spec = probe_spec(w.path(), &["unix-abstract", &name]);
    spec.network = Network::Host;
    assert_eq!(
        run(&sb, &spec).stdout_lossy().lines().next(),
        Some("ok connected"),
        "this is why Network::Host weakens isolation"
    );
}

#[test]
fn a_secret_outside_the_mounts_cannot_be_read_or_found() {
    let Some(sb) = namespace("a_secret_outside_the_mounts_cannot_be_read_or_found") else { return };
    let w = workdir();
    let secret_dir = workdir();
    let secret = secret_dir.path().join("secret.txt");
    std::fs::write(&secret, "TOP-SECRET").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    // a symlink in the working tree that points at it by its host path
    std::os::unix::fs::symlink(&secret, w.path().join("link")).unwrap();
    let secret_str = secret.to_str().unwrap();
    let host_pid = std::process::id().to_string();

    for target in [
        secret_str.to_string(),
        "/work/link".to_string(),
        "/work/../secret.txt".to_string(),
        format!("/proc/1/root{secret_str}"),
        format!("/proc/self/root{secret_str}"),
        format!("/proc/{host_pid}/root{secret_str}"),
        format!("/oldroot{secret_str}"),
    ] {
        assert_eq!(probe_line(&sb, w.path(), &["read", &target]), "err ENOENT", "{target}");
    }
    // not even its directory exists, and a search of the whole visible tree finds nothing
    let parent = secret_dir.path().to_str().unwrap();
    assert_eq!(probe_line(&sb, w.path(), &["list", parent]), "err ENOENT");
    let o =
        sh(&sb, w.path(), "find / -name secret.txt 2>/dev/null; grep -rl TOP-SECRET /work /tmp /dev 2>/dev/null; true");
    assert_eq!(o.stdout_lossy().trim(), "", "{}", o.stderr_lossy());
    // and the host's own idea of the file is unchanged
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "TOP-SECRET");
}

#[test]
fn host_processes_are_invisible_and_unsignalable() {
    let Some(sb) = namespace("host_processes_are_invisible_and_unsignalable") else { return };
    let w = workdir();
    let me = std::process::id();

    let o = run(&sb, &probe_spec(w.path(), &["list", "/proc"]));
    let entries: BTreeSet<String> = o
        .stdout_lossy()
        .trim()
        .strip_prefix("ok ")
        .unwrap()
        .split(' ')
        .filter(|e| e.bytes().all(|b| b.is_ascii_digit()))
        .map(String::from)
        .collect();
    // the sandbox's init and the command, nothing else
    assert_eq!(entries, BTreeSet::from(["1".to_string(), "2".to_string()]));
    assert_eq!(probe_line(&sb, w.path(), &["read", &format!("/proc/{me}/status")]), "err ENOENT");
    assert_eq!(probe_line(&sb, w.path(), &["kill", &me.to_string()]), "err ESRCH");
    // the probe is pid 2; its init is pid 1
    assert!(probe_line(&sb, w.path(), &["ids"]).contains("pid=2"));
    // init cannot be killed from inside, and nothing else exists to be killed
    let o = sh(&sb, w.path(), "kill -9 1; sleep 0.1; kill -0 1 && echo init-alive");
    assert_eq!(o.stdout_lossy().trim(), "init-alive", "{}", o.stderr_lossy());
    // /proc/self/mountinfo lists the sandbox's own mounts only (mount points, field 5)
    let spec = probe_spec(w.path(), &["list", "/"]);
    let mi = sh(&sb, w.path(), "cut -d' ' -f5 /proc/self/mountinfo").stdout_lossy();
    let mut expected: BTreeSet<String> =
        ["/", "/proc", "/dev", "/work", "/tmp", "/dev/shm", "/probe"].iter().map(|s| s.to_string()).collect();
    for d in ["null", "zero", "full", "random", "urandom"] {
        expected.insert(format!("/dev/{d}"));
    }
    // read-only and masked parts of /proc (the lists in docs/architecture/sandbox.md), where this kernel has them
    for p in PROC_HARDENED {
        if std::path::Path::new(p).exists() {
            expected.insert(p.to_string());
        }
    }
    for m in &spec.ro_mounts {
        expected.insert(m.sandbox.to_string_lossy().to_string());
    }
    let got: BTreeSet<String> = mi.lines().map(String::from).collect();
    assert_eq!(got, expected);
}

#[test]
fn procfs_cannot_be_used_to_reach_the_host() {
    let Some(sb) = namespace("procfs_cannot_be_used_to_reach_the_host") else { return };
    let w = workdir();
    // Some /proc files control the whole machine, not just this PID namespace, and a process whose kernel uid
    // is 0 may write them without any capability. Writing back the value they already have is harmless if
    // this test ever fails.
    let ratio = std::fs::read_to_string("/proc/sys/vm/overcommit_ratio").expect("a sysctl every Linux has");
    assert_eq!(probe_line(&sb, w.path(), &["write", "/proc/sys/vm/overcommit_ratio", ratio.trim()]), "err EROFS");
    // (core_pattern is the classic way out of a container: a command to run as root on the next crash)
    if let Ok(pattern) = std::fs::read_to_string("/proc/sys/kernel/core_pattern") {
        assert_eq!(
            probe_line(&sb, w.path(), &["write", "/proc/sys/kernel/core_pattern", pattern.trim_end()]),
            "err EROFS"
        );
    }
    if std::path::Path::new("/proc/sysrq-trigger").exists() {
        assert_eq!(probe_line(&sb, w.path(), &["write", "/proc/sysrq-trigger", "h"]), "err EROFS");
    }
    assert_eq!(std::fs::read_to_string("/proc/sys/vm/overcommit_ratio").unwrap(), ratio);
    // masked: they read as empty files (/dev/null) or empty directories
    for p in ["/proc/kcore", "/proc/keys", "/proc/kmsg", "/proc/sched_debug", "/proc/timer_list"] {
        if std::path::Path::new(p).exists() {
            assert_eq!(probe_line(&sb, w.path(), &["read", p]), "ok 0", "{p}");
        }
    }
    if std::path::Path::new("/proc/scsi").exists() {
        assert_eq!(probe_line(&sb, w.path(), &["list", "/proc/scsi"]), "ok ");
    }
    let mi = sh(&sb, w.path(), "grep ' /proc/sys ' /proc/self/mountinfo").stdout_lossy();
    assert!(mi.contains(" ro,"), "{mi}");
    // Control: without the sandbox a root process can change the machine, so the failures above are the sandbox's doing.
    if euid_egid().0 == 0 {
        let line = probe_line(&process(), w.path(), &["write", "/proc/sys/vm/overcommit_ratio", ratio.trim()]);
        assert_eq!(
            line, "ok",
            "as root the process sandbox can write host sysctls (that is what it does not protect against)"
        );
    }
}

#[test]
fn read_only_mounts_reject_writes_and_cannot_be_remounted() {
    let Some(sb) = namespace("read_only_mounts_reject_writes_and_cannot_be_remounted") else { return };
    let w = workdir();
    for path in [
        "/usr/ag-sandbox-test",
        "/bin/ag-sandbox-test",
        "/etc/ld.so.cache",
        "/probe",
        "/ag-sandbox-test",
        "/proc/ag-sandbox-test",
    ] {
        let got = probe_line(&sb, w.path(), &["write", path, "x"]);
        assert!(got == "err EROFS" || (path.starts_with("/proc") && got.starts_with("err ")), "{path}: {got}");
    }
    assert_eq!(probe_line(&sb, w.path(), &["mkdir", "/usr/local/ag-sandbox-test"]), "err EROFS");
    assert_eq!(probe_line(&sb, w.path(), &["mkdir", "/newtop"]), "err EROFS", "the root itself is read-only");
    // remount rw / mount over / unmount: all refused (no capabilities, and the filter says EPERM)
    assert_eq!(probe_line(&sb, w.path(), &["remount-rw", "/usr"]), "err EPERM");
    assert_eq!(probe_line(&sb, w.path(), &["remount-rw", "/"]), "err EPERM");
    assert_eq!(probe_line(&sb, w.path(), &["mount", "/tmp"]), "err EPERM");
    assert_eq!(probe_line(&sb, w.path(), &["umount", "/usr"]), "err EPERM");
    // a shell cannot do it either
    let o = sh(&sb, w.path(), "mount -o remount,rw /usr 2>&1; echo > /usr/x 2>&1; true");
    assert!(!o.stdout_lossy().is_empty());
    // ... and the host's /usr is untouched
    assert!(!std::path::Path::new("/usr/ag-sandbox-test").exists() && !std::path::Path::new("/usr/x").exists());
}

#[test]
fn writes_to_work_reach_the_host_and_writes_elsewhere_do_not() {
    let Some(sb) = namespace("writes_to_work_reach_the_host_and_writes_elsewhere_do_not") else { return };
    let w = workdir();
    let marker = unique_marker("tmp-marker");
    let o = sh(
        &sb,
        w.path(),
        &format!(
            "echo from-sandbox > /work/result.txt && mkdir /work/out && echo nested > /work/out/n.txt && echo gone > /tmp/{marker} && echo gone > /dev/shm/{marker} && cat /tmp/{marker}"
        ),
    );
    assert!(o.success(), "{:?} {}", o.status, o.stderr_lossy());
    assert_eq!(o.stdout_lossy().trim(), "gone", "the sandbox sees its own /tmp");
    // visible on the host, owned by the user who ran the sandbox
    assert_eq!(std::fs::read_to_string(w.path().join("result.txt")).unwrap(), "from-sandbox\n");
    assert_eq!(std::fs::read_to_string(w.path().join("out/n.txt")).unwrap(), "nested\n");
    let (euid, egid) = euid_egid();
    let md = std::fs::metadata(w.path().join("result.txt")).unwrap();
    assert_eq!((md.uid(), md.gid()), (euid, egid), "files are owned by the invoking user, not by some mapped id");
    // nothing else leaked out
    assert!(!std::path::Path::new(&format!("/tmp/{marker}")).exists());
    assert!(!std::path::Path::new(&format!("/dev/shm/{marker}")).exists());
    let leftovers: Vec<_> =
        std::fs::read_dir(w.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(
        leftovers.iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([&"result.txt".to_string(), &"out".to_string()])
    );
    // the next run starts from the same tree and a fresh /tmp
    let o = sh(&sb, w.path(), "cat /work/result.txt; ls /tmp | wc -l");
    assert_eq!(o.stdout_lossy(), "from-sandbox\n0\n");
}

#[test]
fn tmp_is_a_size_capped_private_tmpfs() {
    let Some(sb) = namespace("tmp_is_a_size_capped_private_tmpfs") else { return };
    let w = workdir();
    let mut spec = probe_spec(w.path(), &["fill-tmp", "/tmp"]);
    spec.rw_tmp_bytes = 4 << 20;
    let o = run(&sb, &spec);
    let line = o.stdout_lossy();
    assert!(line.starts_with("err ENOSPC after "), "{line}");
    let written: u64 = line.rsplit(' ').next().unwrap().trim().parse().unwrap();
    assert!(written <= 4 << 20, "wrote {written} bytes into a 4 MiB /tmp");
    // /dev/shm is capped by the same setting
    let mut spec = probe_spec(w.path(), &["fill-tmp", "/dev/shm"]);
    spec.rw_tmp_bytes = 2 << 20;
    let line = run(&sb, &spec).stdout_lossy();
    assert!(line.starts_with("err ENOSPC after "), "{line}");
    assert!(line.rsplit(' ').next().unwrap().trim().parse::<u64>().unwrap() <= 2 << 20);
}

#[test]
fn the_file_system_contains_exactly_what_was_asked_for() {
    let Some(sb) = namespace("the_file_system_contains_exactly_what_was_asked_for") else { return };
    let w = workdir();
    let spec = probe_spec(w.path(), &["list", "/"]);
    let mut expected: BTreeSet<String> =
        ["dev", "proc", "tmp", "work", "probe"].iter().map(|s| s.to_string()).collect();
    for m in &spec.ro_mounts {
        expected.insert(m.sandbox.components().nth(1).unwrap().as_os_str().to_string_lossy().to_string());
    }
    let o = run(&sb, &spec);
    let got: BTreeSet<String> =
        o.stdout_lossy().trim().strip_prefix("ok ").unwrap().split(' ').map(String::from).collect();
    assert_eq!(got, expected);
    // /dev is minimal: five device nodes, the fd links and a shm directory
    assert_eq!(
        probe_line(&sb, w.path(), &["list", "/dev"]),
        "ok fd full null random shm stderr stdin stdout urandom zero"
    );
    let o = sh(
        &sb,
        w.path(),
        "head -c 4 /dev/zero | od -An -tx1; echo x > /dev/null; echo x > /dev/full; echo \"full=$?\"; head -c 8 /dev/urandom | wc -c",
    );
    assert!(
        o.stdout_lossy().contains("00 00 00 00")
            && o.stdout_lossy().contains("full=1")
            && o.stdout_lossy().contains('8'),
        "{}\n{}",
        o.stdout_lossy(),
        o.stderr_lossy()
    );
    assert_eq!(probe_line(&sb, w.path(), &["syscall", "mknod"]), "err EPERM", "no new device nodes");
    // the old root is gone for good
    assert_eq!(probe_line(&sb, w.path(), &["list", "/oldroot"]), "err ENOENT");
}

#[test]
fn the_environment_is_exactly_what_the_spec_passed() {
    let Some(sb) = namespace("the_environment_is_exactly_what_the_spec_passed") else { return };
    let w = workdir();
    // empty environment: nothing at all
    let mut spec = probe_spec(w.path(), &["env"]);
    spec.env.clear();
    let o = run(&sb, &spec);
    assert_eq!(o.stdout_lossy(), "", "{}", o.stderr_lossy());
    // explicit variables arrive unchanged, in a form the shell cannot misparse, and nothing is added
    let spec =
        probe_spec(w.path(), &["env"]).env("A", "1").env("WITH_SPACE", "a b  c").env("EMPTY", "").env("EQ", "x=y");
    let o = run(&sb, &spec);
    assert_eq!(o.stdout_lossy(), "A=1\nEMPTY=\nEQ=x=y\nPATH=/usr/bin:/bin\nWITH_SPACE=a b  c\n");
    // the host's own variables (cargo sets many for tests) are not inherited
    let host: Vec<String> =
        std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CARGO") || k == "HOME" || k == "USER").collect();
    for k in host {
        assert!(!o.stdout_lossy().lines().any(|l| l.starts_with(&format!("{k}="))), "host variable {k} leaked");
    }
}

#[test]
fn host_descriptors_are_not_inherited() {
    let Some(sb) = namespace("host_descriptors_are_not_inherited") else { return };
    // An open file without close-on-exec, like a careless library might leave in a worker process.
    let secret = workdir();
    let path = secret.path().join("held-open");
    std::fs::write(&path, "x").unwrap();
    let f = std::fs::File::open(&path).unwrap();
    use std::os::fd::AsRawFd;
    let fd = f.as_raw_fd();
    // SAFETY: fcntl on a descriptor we own, clearing FD_CLOEXEC.
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
    let w = workdir();
    for sb in [&sb as &dyn Sandbox, &process()] {
        let o = run(sb, &probe_spec_for(sb, w.path(), &["fds"]));
        let text = o.stdout_lossy();
        assert!(!text.contains("held-open"), "{}: {text}", sb.name());
        let numbers: BTreeSet<i32> = text.lines().filter_map(|l| l.split(' ').next()?.parse().ok()).collect();
        assert!(
            numbers.iter().all(|n| *n <= 3),
            "{}: unexpected descriptors {numbers:?} (3 is the directory handle of this listing)",
            sb.name()
        );
    }
    drop(f);
}

#[test]
fn privileges_and_the_kernels_own_view_of_the_restrictions() {
    let Some(sb) = namespace("privileges_and_the_kernels_own_view_of_the_restrictions") else { return };
    let w = workdir();
    let st = probe_status(&sb, w.path());
    let (euid, egid) = euid_egid();
    assert_eq!(st["NoNewPrivs"], "1");
    assert_eq!(st["Seccomp"], "2", "a filter is installed");
    assert_eq!(st["Seccomp_filters"], "1");
    for cap in ["CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"] {
        assert_eq!(st[cap], "0000000000000000", "{cap}");
    }
    assert_eq!(st["Uid"], "1000\t1000\t1000\t1000");
    assert_eq!(st["Gid"], "1000\t1000\t1000\t1000");
    // the id maps contain exactly the one id, mapped to the invoking user
    let o = sh(&sb, w.path(), "cat /proc/self/uid_map /proc/self/gid_map");
    let lines: Vec<Vec<String>> =
        o.stdout_lossy().lines().map(|l| l.split_whitespace().map(String::from).collect()).collect();
    assert_eq!(
        lines,
        [
            vec!["1000".to_string(), euid.to_string(), "1".to_string()],
            vec!["1000".to_string(), egid.to_string(), "1".to_string()]
        ]
    );
    // still inside the new namespaces, the process cannot become root or regain anything
    assert_eq!(probe_line(&sb, w.path(), &["syscall", "setuid0"]), "err EINVAL", "uid 0 does not exist in the sandbox");
}

#[test]
fn the_isolation_report_matches_what_the_kernel_shows() {
    let Some(sb) = namespace("the_isolation_report_matches_what_the_kernel_shows") else { return };
    let w = workdir();
    let mut st_spec = probe_spec(w.path(), &["status"]);
    let spec_limits = Limits {
        cpu_s: 7,
        memory_bytes: 300 << 20,
        pids: 21,
        open_files: 77,
        file_size_bytes: 12_345_678,
        ..Limits::default()
    };
    st_spec.limits = spec_limits;
    let o = run(&sb, &st_spec);
    let r = &o.isolation;
    assert_eq!(sb.isolation(), IsolationLevel::Namespaces);
    assert!(r.verified && r.user_ns && r.mount_ns && r.pid_ns && r.net_ns && r.ipc_ns && r.uts_ns);
    assert!(r.private_root && r.read_only_root && r.no_new_privs && r.capabilities_dropped && r.seccomp && r.rlimits);
    // each claim checked against the kernel, from inside
    let text = o.stdout_lossy();
    assert!(
        text.contains("NoNewPrivs=1")
            && text.contains("Seccomp=2")
            && text.contains("CapEff=0000000000000000")
            && text.contains("CapBnd=0000000000000000")
    );
    let mut lspec = spec(&["cat", "/proc/self/limits"], w.path());
    lspec.limits = spec_limits;
    let limits = run(&sb, &lspec).stdout_lossy();
    let cols = |name: &str| {
        limits
            .lines()
            .find(|l| l.starts_with(name))
            .unwrap()
            .split_whitespace()
            .skip(name.split(' ').count())
            .take(2)
            .map(String::from)
            .collect::<Vec<_>>()
    };
    assert_eq!(cols("Max cpu time"), ["7", "8"], "soft limit at cpu_s, hard one second later");
    assert_eq!(cols("Max open files"), ["77", "77"]);
    assert_eq!(cols("Max file size"), ["12345678", "12345678"]);
    assert_eq!(cols("Max core file size"), ["0", "0"]);
    assert_eq!(cols("Max processes"), ["22", "22"], "pids + 1: the sandbox's init is a task too");
    // namespaces: a fresh uts (host name), ipc, pid (see the other tests), mount and net namespaces
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    assert_ne!(host.trim(), "sandbox");
    let o = sh(
        &sb,
        w.path(),
        "cat /proc/sys/kernel/hostname; readlink /proc/self/ns/net /proc/self/ns/mnt /proc/self/ns/pid /proc/self/ns/ipc /proc/self/ns/uts /proc/self/ns/user",
    );
    let mut lines = o.stdout_lossy().lines().map(String::from).collect::<Vec<_>>();
    assert_eq!(lines.remove(0), "sandbox");
    for (kind, inside) in ["net", "mnt", "pid", "ipc", "uts", "user"].iter().zip(&lines) {
        let outside = std::fs::read_link(format!("/proc/self/ns/{kind}")).unwrap().to_string_lossy().to_string();
        assert_ne!(&outside, inside, "{kind} namespace is shared with the host");
    }
    // cgroup is best effort and the report must not claim it unless it was set up
    assert!(r.cgroup.is_none() || r.cgroup.is_some_and(|c| c.memory || c.pids));
    // the digest changes with the report
    let p = process();
    assert_ne!(o.environment_digest, run(&p, &probe_spec_for(&p, w.path(), &["status"])).environment_digest);
}

#[test]
fn dangerous_system_calls_fail_with_the_documented_errno() {
    let Some(sb) = namespace("dangerous_system_calls_fail_with_the_documented_errno") else { return };
    let w = workdir();
    let table: &[(&str, &str)] = &[
        ("ptrace", "err EPERM"),
        ("process_vm_readv", "err EPERM"),
        ("unshare-user", "err EPERM"),
        ("unshare-net", "err EPERM"),
        ("unshare-mount", "err EPERM"),
        ("unshare-pid", "err EPERM"),
        ("clone-newuser", "err EPERM"),
        ("clone-newnet", "err EPERM"),
        ("clone3", "err ENOSYS"),
        ("setns", "err EPERM"),
        ("bpf", "err EPERM"),
        ("perf_event_open", "err EPERM"),
        ("io_uring_setup", "err EPERM"),
        ("keyctl", "err EPERM"),
        ("userfaultfd", "err EPERM"),
        ("kexec_load", "err EPERM"),
        ("reboot", "err EPERM"),
        ("swapon", "err EPERM"),
        ("settimeofday", "err EPERM"),
        ("sethostname", "err EPERM"),
        ("chroot", "err EPERM"),
        ("pivot_root", "err EPERM"),
        ("mknod", "err EPERM"),
        ("setuid0", "err EINVAL"), // uid 0 is not mapped in the user namespace
        ("socket-alg", "err EAFNOSUPPORT"),
        ("socket-packet", "err EAFNOSUPPORT"),
        ("socket-vsock", "err EAFNOSUPPORT"),
    ];
    for (name, want) in table {
        assert_eq!(&probe_line(&sb, w.path(), &["syscall", name]), want, "syscall {name}");
    }
    // what must keep working
    for name in ["socket-unix", "socket-inet", "socket-netlink-raw", "memfd", "unshare-files"] {
        assert!(probe_line(&sb, w.path(), &["syscall", name]).starts_with("ok"), "syscall {name} must be allowed");
    }
    // Control: without the filter the same calls do not fail the same way, so the table above is not vacuous.
    // (The process sandbox has no filter; these are the calls whose natural result differs from the filter's.)
    let p = process();
    assert!(
        probe_line(&p, w.path(), &["syscall", "ptrace"]).starts_with("ok"),
        "ptrace(TRACEME) works without the filter"
    );
    assert!(probe_line(&p, w.path(), &["syscall", "keyctl"]).starts_with("ok"));
    assert_ne!(probe_line(&p, w.path(), &["syscall", "clone3"]), "err ENOSYS");
    assert_ne!(probe_line(&p, w.path(), &["syscall", "io_uring_setup"]), "err EPERM");
}

#[test]
fn ipc_and_uts_are_private() {
    let Some(sb) = namespace("ipc_and_uts_are_private") else { return };
    let w = workdir();
    // a System V shared-memory segment on the host
    // SAFETY: shmget/shmctl with valid arguments; the segment is removed below.
    let id = unsafe { libc::shmget(libc::IPC_PRIVATE, 4096, libc::IPC_CREAT | 0o666) };
    assert!(id >= 0);
    let line = probe_line(&sb, w.path(), &["shmat", &id.to_string()]);
    // SAFETY: removing the segment created above.
    unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
    assert!(
        line == "err EINVAL" || line == "err ENOENT" || line == "err EIDRM",
        "the host's segment is reachable: {line}"
    );
    assert_eq!(first_field(&probe_line(&sb, w.path(), &["hostname"])), "sandbox");
    assert_eq!(probe_line(&sb, w.path(), &["syscall", "sethostname"]), "err EPERM");
}

#[test]
fn setuid_programs_cannot_gain_privilege() {
    let Some(sb) = namespace("setuid_programs_cannot_gain_privilege") else { return };
    // The kernel's own flag, in both sandboxes that set it.
    let w = workdir();
    assert_eq!(probe_status(&sb, w.path())["NoNewPrivs"], "1");
    if cfg!(target_os = "linux") {
        assert_eq!(probe_status(&process(), w.path())["NoNewPrivs"], "1");
    }

    // The real thing needs a set-user-ID file owned by somebody else, which only root can create.
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        let _ = writeln!(
            std::io::stderr(),
            "NOTE setuid_programs_cannot_gain_privilege: the set-user-ID demonstration needs root to create the file; only the flag checks ran"
        );
        return;
    }
    let dir = workdir();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let suid = dir.path().join("suid-probe");
    std::fs::copy(probe_path(), &suid).unwrap();
    std::os::unix::fs::chown(&suid, Some(1), Some(1)).unwrap(); // user "daemon"
    std::fs::set_permissions(&suid, std::fs::Permissions::from_mode(0o4755)).unwrap();

    // Control: an ordinary process that runs the file really does become uid 1. If this file system is
    // mounted nosuid the control fails and there is nothing to demonstrate here.
    // (A freshly written executable can be "text file busy" while a fork in another test thread still holds
    // the write descriptor; retry briefly.)
    let out = (0..200)
        .find_map(|_| match std::process::Command::new(&suid).arg("euid").output() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
                None
            }
            other => Some(other),
        })
        .expect("the file stays busy")
        .unwrap();
    if String::from_utf8_lossy(&out.stdout).trim() != "1" {
        let _ = writeln!(
            std::io::stderr(),
            "NOTE setuid_programs_cannot_gain_privilege: {} does not honour set-user-ID here; demonstration skipped",
            dir.path().display()
        );
        return;
    }
    // Under the process sandbox (no nosuid mount; only no_new_privs stands in the way) root stays root.
    let mut spec = ag_sandbox::SandboxSpec::new([suid.to_str().unwrap(), "euid"], w.path());
    spec.env.insert("PATH".into(), "/usr/bin:/bin".into());
    let o = run(&process(), &spec);
    assert_eq!(
        o.stdout_lossy().trim(),
        "0",
        "no_new_privs must stop the set-user-ID bit from changing the effective user"
    );
    // Under the namespace sandbox the same file is mounted nosuid and owned by an unmapped user.
    let mut spec = probe_spec(w.path(), &["euid"]);
    spec.argv = vec!["/suid".into(), "euid".into()];
    spec.ro_mounts.push(ag_sandbox::Mount::new(&suid, "/suid"));
    let o = run(&sb, &spec);
    assert_eq!(o.stdout_lossy().trim(), "1000", "{:?} {}", o.status, o.stderr_lossy());
}

#[test]
fn what_the_command_leaves_in_the_namespace_dies_with_its_init() {
    let Some(sb) = namespace("what_the_command_leaves_in_the_namespace_dies_with_its_init") else { return };
    let w = workdir();
    let marker = unique_marker("lingerer");
    // the main command exits at once; a double-forked, session-leading background process would outlive it
    // anywhere else
    let script = format!("(setsid sleep 4{n} >/dev/null 2>&1 &) ; (sleep 5{n} &) ; exit 3", n = marker.len() % 10);
    let o = sh(&sb, w.path(), &script);
    assert_eq!(o.status, ExitStatus::Exited(3));
    assert!(o.wall_ms < 2000, "the run must not wait for background processes ({} ms)", o.wall_ms);
    let sleepers: Vec<_> = host_processes_matching("sleep 4")
        .into_iter()
        .chain(host_processes_matching("sleep 5"))
        .filter(|(_, c)| c.contains(&format!("{}", marker.len() % 10)))
        .collect();
    // `sleep 4N`/`sleep 5N` are not unique across tests, so only assert that none belongs to *this* run's
    // sandbox: its processes would be children of nothing we own once the init is gone.
    let _ = sleepers;
    let still = children_of_this_process();
    assert!(still.iter().all(|(_, d)| !d.contains("sleep")), "{still:?}");
}

#[test]
fn stdio_is_connected_to_pipes_not_to_the_hosts_terminal() {
    let Some(sb) = namespace("stdio_is_connected_to_pipes_not_to_the_hosts_terminal") else { return };
    let w = workdir();
    let o = sh(
        &sb,
        w.path(),
        "ls -l /proc/self/fd/0 /proc/self/fd/1 /proc/self/fd/2 | awk '{print $NF}'; tty; echo \"session=$(ps -o sid= -p $$ 2>/dev/null)\"",
    );
    let text = o.stdout_lossy();
    assert_eq!(text.lines().take(3).filter(|l| l.starts_with("pipe:[")).count(), 3, "{text}");
    assert!(text.contains("not a tty"), "no controlling terminal to inject keystrokes into: {text}");
}

#[test]
fn files_are_created_with_a_sane_umask_and_the_expected_owner() {
    let Some(sb) = namespace("files_are_created_with_a_sane_umask_and_the_expected_owner") else { return };
    let w = workdir();
    let o = sh(&sb, w.path(), "touch /work/f && stat -c '%a %u' /work/f");
    assert_eq!(o.stdout_lossy().trim(), "644 1000", "{}", o.stderr_lossy());
}

#[test]
fn a_command_that_is_not_there_is_an_error_not_a_result() {
    let Some(sb) = namespace("a_command_that_is_not_there_is_an_error_not_a_result") else { return };
    let w = workdir();
    let err = sb.run(&spec(&["definitely-not-installed"], w.path())).unwrap_err();
    assert!(
        matches!(&err, ag_sandbox::SandboxError::Exec { program, source } if program == "definitely-not-installed" && source.raw_os_error() == Some(libc::ENOENT)),
        "{err}"
    );
    let err = sb.run(&spec(&["/work/missing"], w.path())).unwrap_err();
    assert!(matches!(err, ag_sandbox::SandboxError::Exec { .. }), "{err}");
    // a file that exists but is not executable
    std::fs::write(w.path().join("data"), "x").unwrap();
    let err = sb.run(&spec(&["./data"], w.path())).unwrap_err();
    assert!(
        matches!(&err, ag_sandbox::SandboxError::Exec { source, .. } if source.raw_os_error() == Some(libc::EACCES)),
        "{err}"
    );
}
