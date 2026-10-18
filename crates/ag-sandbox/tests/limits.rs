//! Resource limits and process control: timeouts that kill everything, contained fork bombs, memory and CPU
//! limits, output caps, stdin delivery, exit-status mapping, and survival of the sandbox when its host dies.
#![allow(unsafe_code)] // the host-death test kills a helper process
#![cfg(target_os = "linux")] // the namespace and container sandboxes are exercised on Linux only

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use std::io::Write as _;

use ag_sandbox::{Enforcement, ExitStatus, Limits, Sandbox, SandboxError, SandboxSpec};
use common::*;

/// A decimal string that identifies one `sleep` among all processes on the machine.
fn sleep_marker() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!("{}{:03}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn the_wall_clock_budget_kills_the_command_and_every_descendant() {
    let Some(sb) = namespace("the_wall_clock_budget_kills_the_command_and_every_descendant") else { return };
    let w = workdir();
    let m = sleep_marker();
    let (a, b, c, d) = (format!("1{m}"), format!("2{m}"), format!("3{m}"), format!("4{m}"));
    // a child, a grandchild, a session leader that left the process group, and a daemon whose parent exited
    let script = format!("sleep {a} & sleep {b} & (setsid sleep {c} &) ; (sleep {d} &) ; wait");
    let mut spec = spec(&["sh", "-c", &script], w.path());
    spec.limits.wall_ms = 700;
    let started = Instant::now();
    let o = run(&sb, &spec);
    assert_eq!(o.status, ExitStatus::TimedOut);
    assert!(o.wall_ms >= 700 && o.wall_ms < 5000, "wall_ms={}", o.wall_ms);
    assert!(started.elapsed() < Duration::from_secs(10));
    // from the outside: not one of them is left, not even as a zombie (the whole namespace is reaped)
    for id in [&a, &b, &c, &d] {
        let left = host_processes_matching(&format!("sleep {id}"));
        assert!(left.is_empty(), "survivors of `sleep {id}`: {left:?}");
    }
}

#[test]
fn background_processes_do_not_outlive_the_main_command() {
    let Some(sb) = namespace("background_processes_do_not_outlive_the_main_command") else { return };
    let w = workdir();
    let m = sleep_marker();
    let script = format!("(sleep 1{m} &); (setsid sleep 2{m} >/dev/null 2>&1 &); exit 3");
    let o = run(&sb, &spec(&["sh", "-c", &script], w.path()));
    assert_eq!(o.status, ExitStatus::Exited(3));
    assert!(o.wall_ms < 3000, "the run must not wait for the daemons ({} ms)", o.wall_ms);
    for id in [format!("1{m}"), format!("2{m}")] {
        assert!(host_processes_matching(&format!("sleep {id}")).is_empty(), "sleep {id} outlived the sandbox");
    }
}

/// Fork bombs against `sb`, which must enforce `pids` by whatever means it reports.
fn fork_bombs_are_contained(sb: &dyn Sandbox, workdir: &std::path::Path, test: &str) {
    let w = workdir;
    let pids = 20;
    let limits = Limits { pids, ..Limits::default() };

    // Processes, then threads: how many the probe manages to create, as seen from the inside.
    for (cmd, noun) in [("forkbomb", "forked"), ("threadbomb", "threads")] {
        let mut s = probe_spec(w, &[cmd, "100000"]);
        s.limits = limits;
        let o = run(sb, &s);
        match o.isolation.tasks {
            Enforcement::Rlimit | Enforcement::Cgroup => {
                // The kernel refuses the surplus fork: exact. The probe is task 1 of `pids`.
                let line = o.stdout_lossy();
                let n: u64 = line.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or_else(|| {
                    panic!("{test}: no count in {line:?} (status {:?}, stderr {})", o.status, o.stderr_lossy())
                });
                assert!(line.contains("stopped_by EAGAIN"), "{test}: {line}");
                let created = n + 1;
                assert!(
                    created <= pids && created + 3 >= pids,
                    "{test}: {noun} {n} (+ the probe itself) against a limit of {pids}"
                );
            }
            Enforcement::Sampled => {
                assert_eq!(o.status, ExitStatus::PidsLimit, "{test}: {cmd}: {}", o.stdout_lossy());
            }
            other => panic!("{test}: the task limit is not enforced at all ({other:?})"),
        }
    }

    // Real shell bombs, watched from the outside while they run: the classic recursive one, and a loop that
    // keeps starting sleepers (which stays at the limit until the timeout).
    for (kind, grows) in [("recursive", false), ("spawner", true)] {
        let marker = format!("fb{}x{}", std::process::id(), sleep_marker());
        let mut s = if grows {
            // marker is an argument, so every forked copy carries it in its command line
            probe_spec(w, &["spawner", &marker])
        } else {
            let script = format!("{marker}() {{ {marker} | {marker} & }}; {marker}; wait");
            spec(&["sh", "-c", &script], w)
        };
        s.limits = Limits { pids: 30, wall_ms: 1500, ..Limits::default() };
        let (peak, status, tasks) = std::thread::scope(|scope| {
            let runner = scope.spawn(|| run(sb, &s));
            let mut peak = 0;
            while !runner.is_finished() {
                peak = peak.max(host_processes_matching(&marker).len());
                std::thread::sleep(Duration::from_millis(1));
            }
            let o = runner.join().unwrap();
            (peak, o.status, o.isolation.tasks)
        });
        match tasks {
            Enforcement::Rlimit | Enforcement::Cgroup => {
                if grows {
                    assert_eq!(status, ExitStatus::TimedOut, "{test}: the spawner never finishes by itself");
                    assert!(
                        peak >= 20,
                        "{test}: the {kind} bomb should have grown (saw {peak}); otherwise this proves nothing"
                    );
                }
                // the recursive one either ends once its forks fail (usual) or spins until the timeout
                assert!(matches!(status, ExitStatus::Exited(_) | ExitStatus::TimedOut), "{test}: {kind}: {status:?}");
                assert!(peak <= 30, "{test}: {peak} {kind} bomb processes existed at once, limit was 30");
            }
            _ => {
                // Sampled: killed as soon as a sample sees too many; the overshoot is bounded by the sampling
                // interval, not by the limit.
                if grows {
                    assert_eq!(status, ExitStatus::PidsLimit, "{test}: {kind}");
                } else {
                    // the recursive bomb's shells exit right after forking, and the sandbox ends with its main
                    // process; both outcomes are containment
                    assert!(
                        matches!(status, ExitStatus::Exited(0) | ExitStatus::PidsLimit),
                        "{test}: {kind}: {status:?}"
                    );
                }
                assert!(peak < 5000, "{test}: {peak} {kind} bomb processes existed at once");
            }
        }
        assert!(host_processes_matching(&marker).is_empty(), "{test}: {kind} bomb processes remain after the run");
    }
}

/// A sandbox whose task limit the kernel enforces exactly, and a working directory it may write.
///
/// For a non-root caller that is the default. For root the kernel exempts uid 0 from `RLIMIT_NPROC`, so the
/// command is mapped to an unprivileged host user instead (which is also what a root worker should do in
/// production).
fn exact_sandbox(test: &str) -> Option<(ag_sandbox::NamespaceSandbox, tempfile::TempDir)> {
    namespace(test)?;
    let w = workdir();
    if euid_egid().0 != 0 {
        return Some((ag_sandbox::NamespaceSandbox::new(), w));
    }
    std::os::unix::fs::chown(w.path(), Some(65534), Some(65534)).unwrap();
    Some((ag_sandbox::NamespaceSandbox::new().with_host_ids(65534, 65534), w))
}

#[test]
fn a_fork_bomb_is_contained_by_the_pids_limit() {
    let test = "a_fork_bomb_is_contained_by_the_pids_limit";
    let Some((sb, w)) = exact_sandbox(test) else { return };
    let o = run(&sb, &spec(&["true"], w.path()));
    assert!(matches!(o.isolation.tasks, Enforcement::Rlimit | Enforcement::Cgroup), "{test}: {:?}", o.isolation.tasks);
    fork_bombs_are_contained(&sb, w.path(), test);
}

#[test]
fn a_root_host_maps_the_command_to_an_unprivileged_user_on_request() {
    let test = "a_root_host_maps_the_command_to_an_unprivileged_user_on_request";
    let Some((sb, w)) = exact_sandbox(test) else { return };
    if euid_egid().0 != 0 {
        let _ = writeln!(
            std::io::stderr(),
            "NOTE {test}: only meaningful for a root host; other users already run as themselves"
        );
        // ... but they cannot ask for somebody else's ids
        let other = ag_sandbox::NamespaceSandbox::new().with_host_ids(0, 0);
        assert!(matches!(other.run(&spec(&["true"], w.path())), Err(SandboxError::InvalidSpec(_))));
        return;
    }
    let o = run(&sb, &spec(&["sh", "-c", "touch /work/f; stat -c '%u %a' /work/f; id -u"], w.path()));
    assert_eq!(o.stdout_lossy(), "1000 644\n1000\n", "{}", o.stderr_lossy());
    let md = std::fs::metadata(w.path().join("f")).unwrap();
    assert_eq!(std::os::unix::fs::MetadataExt::uid(&md), 65534, "files belong to the unprivileged host user");
    assert!(!o.isolation.host_root);
    assert!(matches!(o.isolation.tasks, Enforcement::Rlimit | Enforcement::Cgroup), "{:?}", o.isolation.tasks);
    // the kernel-level identity: not root, so it cannot write the root-owned files it can see
    let o = run(&sb, &probe_spec(w.path(), &["write", "/proc/sys/vm/overcommit_ratio", "50"]));
    assert!(o.stdout_lossy().starts_with("err "), "{}", o.stdout_lossy());
    // and the default configuration says plainly that the command is host root
    let default = run(&ag_sandbox::NamespaceSandbox::new(), &spec(&["true"], w.path()));
    assert!(default.isolation.host_root);
    assert_ne!(default.environment_digest, o.environment_digest);
}

#[test]
fn the_task_limit_is_sampled_where_the_kernel_exempts_the_user() {
    let test = "the_task_limit_is_sampled_where_the_kernel_exempts_the_user";
    let Some(sb) = namespace(test) else { return };
    if euid_egid().0 != 0 {
        let _ =
            writeln!(std::io::stderr(), "NOTE {test}: the kernel enforces the limit for this user; nothing is sampled");
        return;
    }
    // Root, default mapping: the command is kernel uid 0 and RLIMIT_NPROC does not apply to it. The probe
    // forks up to a bounded number of children (never a real bomb on a shared machine), slowly enough for a sample to see it; the supervisor must
    // notice that it went past `pids` and end the run.
    let w = workdir();
    let mut s = probe_spec(w.path(), &["forkbomb", "2000", "300"]);
    s.limits.pids = 20;
    let o = run(&sb, &s);
    assert!(o.isolation.host_root);
    if o.isolation.tasks == Enforcement::Cgroup {
        // a delegated pids cgroup (see tests/cgroup_real.rs) enforces it exactly instead: the surplus fork fails
        let line = o.stdout_lossy();
        assert!(line.contains("stopped_by EAGAIN"), "{line}");
        return;
    }
    assert_eq!(o.isolation.tasks, Enforcement::Sampled);
    assert_eq!(o.status, ExitStatus::PidsLimit, "stdout: {}", o.stdout_lossy());
    // a command that stays within the limit is not disturbed
    let mut s = probe_spec(w.path(), &["forkbomb", "10"]);
    s.limits.pids = 20;
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::Exited(0));
    assert!(o.stdout_lossy().starts_with("forked 10 "), "{}", o.stdout_lossy());
}

#[test]
fn a_memory_hog_is_stopped_by_the_memory_limit() {
    let Some(sb) = namespace("a_memory_hog_is_stopped_by_the_memory_limit") else { return };
    let w = workdir();
    let limit = 128u64 << 20;
    let mut s = probe_spec(w.path(), &["alloc", "4000", "1"]);
    s.limits = Limits { memory_bytes: limit, ..Limits::default() };
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::MemoryLimit, "stdout: {}", o.stdout_lossy());
    let reached: u64 =
        o.stdout_lossy().lines().filter_map(|l| l.strip_prefix("allocated ")?.trim().parse().ok()).max().unwrap_or(0);
    assert!(reached >= 64, "the hog should have got going (reached {reached} MiB)");
    assert!(reached <= 128 + 128, "the hog reached {reached} MiB against a 128 MiB limit");
    assert!(matches!(o.isolation.memory, ag_sandbox::Enforcement::Sampled | ag_sandbox::Enforcement::Cgroup));
    let peak = o.max_rss_bytes.expect("the peak is reported");
    assert!(peak >= limit / 2, "peak {peak}");
}

#[test]
fn memory_spread_over_many_processes_counts_together() {
    let Some(sb) = namespace("memory_spread_over_many_processes_counts_together") else { return };
    let w = workdir();
    // six processes of 40 MiB each: every one is far below the limit, together they are not
    let mut s = probe_spec(w.path(), &["alloc-procs", "6", "40"]);
    s.limits = Limits { memory_bytes: 128 << 20, wall_ms: 20_000, ..Limits::default() };
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::MemoryLimit, "{}", o.stdout_lossy());
}

#[test]
fn a_program_within_its_memory_limit_is_left_alone_and_measured() {
    let Some(sb) = namespace("a_program_within_its_memory_limit_is_left_alone_and_measured") else { return };
    let w = workdir();
    let mut s = probe_spec(w.path(), &["alloc", "48", "3"]);
    s.limits = Limits { memory_bytes: 256 << 20, ..Limits::default() };
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::Exited(0), "{} {}", o.stdout_lossy(), o.stderr_lossy());
    let peak = o.max_rss_bytes.expect("peak");
    assert!((40 << 20..120 << 20).contains(&peak), "peak {peak}");
}

#[test]
fn a_busy_loop_runs_into_the_cpu_limit() {
    let Some(sb) = namespace("a_busy_loop_runs_into_the_cpu_limit") else { return };
    let w = workdir();
    let mut s = probe_spec(w.path(), &["spin"]);
    s.limits = Limits { cpu_s: 1, wall_ms: 30_000, ..Limits::default() };
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::CpuLimit);
    assert!(o.wall_ms < 10_000, "{} ms", o.wall_ms);
    assert!(o.cpu_ms.unwrap_or(0) >= 700, "{:?}", o.cpu_ms);
}

#[test]
fn flooding_stdout_or_stderr_is_capped_and_ends_the_run() {
    let Some(sb) = namespace("flooding_stdout_or_stderr_is_capped_and_ends_the_run") else { return };
    let w = workdir();
    let cap = 64 * 1024u64;
    let limits = Limits { output_bytes: cap, ..Limits::default() };

    let mut s = probe_spec(w.path(), &["flood", "104857600"]);
    s.limits = limits;
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::OutputLimit);
    assert_eq!(o.stdout.len() as u64, cap);
    assert!(o.stdout_truncated && !o.stderr_truncated);
    assert!(o.stdout.iter().all(|&b| b == b'x'));
    assert!(o.wall_ms < 5000);

    let mut s = probe_spec(w.path(), &["flood", "104857600", "stderr"]);
    s.limits = limits;
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::OutputLimit);
    assert_eq!(o.stderr.len() as u64, cap);
    assert!(o.stderr_truncated && !o.stdout_truncated);

    // an endless producer from the system's tools, too
    let mut s = spec(&["yes"], w.path());
    s.limits = limits;
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::OutputLimit);
    assert!(o.wall_ms < 5000);

    // exactly at the limit is fine; one byte more is not
    let mut s = probe_spec(w.path(), &["flood", &cap.to_string()]);
    s.limits = limits;
    let o = run(&sb, &s);
    assert_eq!((o.status, o.stdout.len() as u64, o.stdout_truncated), (ExitStatus::Exited(0), cap, false));
    let mut s = probe_spec(w.path(), &["flood", &(cap + 1).to_string()]);
    s.limits = limits;
    let o = run(&sb, &s);
    assert_eq!((o.status, o.stdout.len() as u64, o.stdout_truncated), (ExitStatus::OutputLimit, cap, true));
}

#[test]
fn stdin_is_delivered_completely_and_never_blocks_the_run() {
    let Some(sb) = namespace("stdin_is_delivered_completely_and_never_blocks_the_run") else { return };
    let w = workdir();
    // bigger than any pipe buffer, with a checksum
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let sum: u64 = data.iter().map(|&b| u64::from(b)).sum();
    let o = run(&sb, &probe_spec(w.path(), &["stdin-sum"]).stdin(data.clone()));
    assert_eq!(o.stdout_lossy().trim(), format!("len=3000000 sum={sum}"), "{:?} {}", o.status, o.stderr_lossy());
    // `cat` echoes it back through stdout (capped generously)
    let mut s = spec(&["cat"], w.path()).stdin(b"hello\nworld\n".to_vec());
    s.limits.output_bytes = 1 << 20;
    assert_eq!(run(&sb, &s).stdout_lossy(), "hello\nworld\n");
    // no stdin at all: immediate end-of-file
    let o = run(&sb, &spec(&["cat"], w.path()));
    assert_eq!((o.status, o.stdout.len()), (ExitStatus::Exited(0), 0));
    // a command that never reads a large stdin does not deadlock the supervisor
    let s = spec(&["sh", "-c", "sleep 0.2; echo done"], w.path()).stdin(vec![b'q'; 5 << 20]);
    let o = run(&sb, &s);
    assert_eq!((o.status, o.stdout_lossy().trim().to_string()), (ExitStatus::Exited(0), "done".to_string()));
    // one that closes it early (head) is fine too
    let s = spec(&["head", "-c", "10"], w.path()).stdin(vec![b'z'; 5 << 20]);
    let o = run(&sb, &s);
    assert_eq!((o.status, o.stdout.len()), (ExitStatus::Exited(0), 10));
}

#[test]
fn exit_codes_and_signals_are_mapped() {
    let Some(sb) = namespace("exit_codes_and_signals_are_mapped") else { return };
    let w = workdir();
    for (script, want) in [
        ("exit 0", ExitStatus::Exited(0)),
        ("exit 7", ExitStatus::Exited(7)),
        ("exit 255", ExitStatus::Exited(255)),
        ("false", ExitStatus::Exited(1)),
        ("kill -9 $$", ExitStatus::Signaled(9)),
        ("kill -TERM $$", ExitStatus::Signaled(15)),
    ] {
        assert_eq!(run(&sb, &spec(&["sh", "-c", script], w.path())).status, want, "{script}");
    }
    assert_eq!(run(&sb, &probe_spec(w.path(), &["raise", "11"])).status, ExitStatus::Signaled(11));
    assert_eq!(run(&sb, &probe_spec(w.path(), &["exit", "42"])).status, ExitStatus::Exited(42));
    assert!(run(&sb, &spec(&["true"], w.path())).success());
    assert!(!run(&sb, &spec(&["false"], w.path())).success());
}

#[test]
fn file_and_descriptor_limits_apply_to_the_command() {
    let Some(sb) = namespace("file_and_descriptor_limits_apply_to_the_command") else { return };
    let w = workdir();
    let mut s = probe_spec(w.path(), &["openfiles"]);
    s.limits.open_files = 24;
    let line = run(&sb, &s).stdout_lossy();
    let n: u64 = line.rsplit(' ').next().unwrap().trim().parse().unwrap();
    assert!(line.starts_with("err EMFILE after ") && (10..=24).contains(&n), "{line}");

    let mut s = probe_spec(w.path(), &["fsize", "/work/big", "5000000"]);
    s.limits.file_size_bytes = 1_000_000;
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::Signaled(25), "SIGXFSZ ends a process that writes past the limit");
    let size = std::fs::metadata(w.path().join("big")).unwrap().len();
    assert!((900_000..=1_000_000).contains(&size), "the file grew to {size} bytes");
}

#[test]
fn invalid_specs_are_rejected_without_running_anything() {
    let Some(sb) = namespace("invalid_specs_are_rejected_without_running_anything") else { return };
    let w = workdir();
    let bad = |s: SandboxSpec| sb.run(&s).unwrap_err();
    assert!(matches!(bad(SandboxSpec::new(Vec::<String>::new(), w.path())), SandboxError::InvalidSpec(_)));
    assert!(matches!(bad(spec(&["true"], w.path()).rw_tmp_bytes(0)), SandboxError::InvalidSpec(_)));
    let mut s = spec(&["true"], w.path());
    s.limits.wall_ms = 0;
    assert!(matches!(bad(s), SandboxError::InvalidSpec(_)));
    let s = spec(&["true"], w.path()).ro_mount(ag_sandbox::Mount::new("/usr", "/work/inside"));
    assert!(matches!(bad(s), SandboxError::InvalidSpec(_)));
    let s = spec(&["true"], w.path()).ro_mount(ag_sandbox::Mount::new("/no/such/path", "/x"));
    assert!(matches!(bad(s), SandboxError::InvalidSpec(_)));
}

#[test]
fn concurrent_runs_from_a_multi_threaded_process() {
    let Some(sb) = namespace("concurrent_runs_from_a_multi_threaded_process") else { return };
    // Regression test: the child created by `clone` has the parent's memory image but only one thread, so
    // anything that waits for other threads (glibc's `setgroups`, `setuid`, ...) or takes a lock another
    // thread held at that instant hangs forever. Run many sandboxes from many threads while others churn
    // the allocator and spawn threads of their own.
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let mut v: Vec<Vec<u8>> = Vec::new();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    v.push(vec![7u8; 4096]);
                    if v.len() > 2000 {
                        v.clear();
                    }
                    let _ = std::thread::spawn(|| ()).join();
                }
            });
        }
        let runners: Vec<_> = (0..8)
            .map(|i| {
                let sb = &sb;
                scope.spawn(move || {
                    let w = workdir();
                    for round in 0..6 {
                        let o = run(sb, &spec(&["sh", "-c", &format!("echo {i}-{round}")], w.path()));
                        assert_eq!(o.stdout_lossy().trim(), format!("{i}-{round}"));
                        assert!(o.wall_ms < 10_000, "run took {} ms", o.wall_ms);
                    }
                })
            })
            .collect();
        for r in runners {
            r.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Not a test by itself: the body of the helper process started by `the_sandbox_dies_with_its_host`.
#[test]
fn helper_host_process_for_the_host_death_test() {
    let Ok(m) = std::env::var("AG_SANDBOX_HELPER_MARKER") else { return };
    let w = workdir();
    let sb = ag_sandbox::NamespaceSandbox::new();
    println!("started");
    let mut s = spec(&["sh", "-c", &format!("sleep {m} & sleep {m}; true")], w.path());
    s.limits.wall_ms = 120_000;
    let _ = sb.run(&s);
}

#[test]
fn the_sandbox_dies_with_its_host() {
    let Some(_sb) = namespace("the_sandbox_dies_with_its_host") else { return };
    let m = format!("5{}", sleep_marker());
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper_host_process_for_the_host_death_test", "--nocapture", "--test-threads=1"])
        .env("AG_SANDBOX_HELPER_MARKER", &m)
        .env("AG_SANDBOX_PROBE", probe_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(helper.stdout.take().unwrap()).lines();
    while let Some(Ok(l)) = lines.next() {
        if l.trim().ends_with("started") {
            break;
        }
    }
    wait_until("the helper's sandbox to start", Duration::from_secs(20), || {
        host_processes_matching(&format!("sleep {m}")).len() >= 2
    });
    // The host process is killed without any chance to clean up.
    helper.kill().unwrap();
    let _ = helper.wait();
    wait_until("the sandbox to die with its host", Duration::from_secs(10), || {
        host_processes_matching(&format!("sleep {m}")).is_empty()
    });
}

#[test]
fn the_reported_wall_time_and_cpu_time_are_plausible() {
    let Some(sb) = namespace("the_reported_wall_time_and_cpu_time_are_plausible") else { return };
    let w = workdir();
    let o = run(&sb, &spec(&["sh", "-c", "sleep 0.3"], w.path()));
    assert!(o.wall_ms >= 300 && o.wall_ms < 5000, "{}", o.wall_ms);
    assert!(o.cpu_ms.unwrap() < 300);
    assert!(o.environment_digest.starts_with("sha256:"));
}
