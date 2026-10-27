//! The namespace sandbox's cgroup v2 limits against a **real** kernel hierarchy.
//!
//! `src/cgroup.rs` has unit tests against a fake directory; those prove the file handling and nothing about
//! the kernel. These tests need a cgroup v2 directory the test user may write to, with the memory and pids
//! controllers enabled for its children and no processes of its own (the `OCI runtimes and cgroup v2`
//! workflow prepares one and starts the tests inside the same delegation):
//!
//! * `AG_SANDBOX_CGROUP_BASE` — that directory (the library reads it too, so the other namespace suites run
//!   with cgroup limits when it is set);
//! * `AG_REQUIRE_COMPONENTS=cgroup-v2` — a host without it fails instead of skipping. CI sets it.
//!
//! A host without a delegated cgroup leaves an `AG-SKIP` marker and passes: it has verified nothing.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ag_sandbox::{CgroupReport, Enforcement, ExitStatus, Limits, NamespaceSandbox};
use common::*;

static LOCK: Mutex<()> = Mutex::new(());

struct Delegated {
    sandbox: NamespaceSandbox,
    base: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn delegated(test: &str) -> Option<Delegated> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sandbox = namespace(test)?;
    let base = std::env::var_os("AG_SANDBOX_CGROUP_BASE").map(PathBuf::from);
    let problem = match sandbox.cgroup_availability() {
        Ok(CgroupReport { memory: true, pids: true }) => None,
        Ok(partial) => Some(format!("only part of the limits can be set: {partial:?}")),
        Err(why) => Some(why),
    };
    if let Some(why) = problem {
        ag_testkit::skip("cgroup-v2", test, &format!("no usable delegated cgroup v2 directory: {why}"));
        return None;
    }
    let base = base.unwrap_or_else(|| PathBuf::from("/nonexistent"));
    Some(Delegated { sandbox, base, _guard: guard })
}

impl Delegated {
    /// The cgroups of this process's runs that exist right now.
    fn cgroups(&self) -> Vec<PathBuf> {
        let prefix = format!("ag-sandbox-{}-", std::process::id());
        std::fs::read_dir(&self.base)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect()
    }
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| format!("unreadable: {e}")).trim().to_string()
}

#[test]
fn a_run_gets_a_real_cgroup_with_the_requested_limits_and_it_is_gone_afterwards() {
    let Some(d) = delegated("a_run_gets_a_real_cgroup_with_the_requested_limits_and_it_is_gone_afterwards") else {
        return;
    };
    assert!(d.cgroups().is_empty(), "nothing is left from earlier runs: {:?}", d.cgroups());
    let w = workdir();
    let limit = 192u64 << 20;
    let mut s = probe_spec(w.path(), &["sleep", "2500"]);
    s.limits = Limits { memory_bytes: limit, pids: 77, ..Limits::default() };
    let (outcome, seen) = std::thread::scope(|scope| {
        let runner = scope.spawn(|| run(&d.sandbox, &s));
        // while the command runs, read the kernel's own files of its cgroup
        let end = Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while Instant::now() < end && !runner.is_finished() {
            if let Some(dir) = d.cgroups().first() {
                let procs = read(dir, "cgroup.procs");
                if !procs.is_empty() && !procs.starts_with("unreadable") {
                    seen = Some((
                        read(dir, "memory.max"),
                        read(dir, "pids.max"),
                        read(dir, "memory.swap.max"),
                        read(dir, "memory.oom.group"),
                        procs,
                    ));
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        (runner.join().unwrap(), seen)
    });
    let (memory_max, pids_max, swap_max, oom_group, procs) = seen.expect("the run's cgroup existed while it ran");
    assert_eq!(memory_max, limit.to_string(), "memory.max");
    assert_eq!(pids_max, "77", "pids.max");
    assert_eq!(swap_max, "0", "no swap");
    assert_eq!(oom_group, "1", "an OOM kill takes the whole group");
    assert!(!procs.is_empty(), "the sandbox's processes are in it");

    assert_eq!(outcome.status, ExitStatus::Exited(0));
    let r = &outcome.isolation;
    assert_eq!(r.cgroup, Some(CgroupReport { memory: true, pids: true }), "{:?}", r.notes);
    assert_eq!((r.memory, r.tasks), (Enforcement::Cgroup, Enforcement::Cgroup));
    assert!(outcome.max_rss_bytes.is_some(), "the peak comes from memory.peak");
    assert!(!r.notes.iter().any(|n| n.contains("no cgroup v2 limits")), "{:?}", r.notes);
    assert!(d.cgroups().is_empty(), "the cgroup was removed: {:?}", d.cgroups());
}

#[test]
fn a_memory_hog_is_killed_by_the_kernels_oom_killer_and_reported_as_the_memory_limit() {
    let Some(d) = delegated("a_memory_hog_is_killed_by_the_kernels_oom_killer_and_reported_as_the_memory_limit") else {
        return;
    };
    let w = workdir();
    let limit = 96u64 << 20;
    let mut s = probe_spec(w.path(), &["alloc", "4000", "1"]);
    s.limits = Limits { memory_bytes: limit, ..Limits::default() };
    let o = run(&d.sandbox, &s);
    assert_eq!(o.isolation.memory, Enforcement::Cgroup, "{:?}", o.isolation.notes);
    assert_eq!(o.status, ExitStatus::MemoryLimit, "stdout: {}", o.stdout_lossy());
    let reached: u64 =
        o.stdout_lossy().lines().filter_map(|l| l.strip_prefix("allocated ")?.trim().parse().ok()).max().unwrap_or(0);
    assert!(reached >= 48, "the hog got going (reached {reached} MiB)");
    // exact, not sampled: the kernel refuses the page that crosses the limit
    assert!(reached <= 96, "the hog was stopped at the limit, not later (reached {reached} MiB of 96)");
    let peak = o.max_rss_bytes.expect("memory.peak");
    assert!(peak >= limit / 2 && peak <= limit, "peak {peak} within the limit {limit}");
    assert!(d.cgroups().is_empty(), "{:?}", d.cgroups());
}

#[test]
fn memory_spread_over_many_processes_is_one_budget_in_the_cgroup() {
    let Some(d) = delegated("memory_spread_over_many_processes_is_one_budget_in_the_cgroup") else { return };
    let w = workdir();
    // six processes of 40 MiB: each is far below the limit, together they are not
    let mut s = probe_spec(w.path(), &["alloc-procs", "6", "40"]);
    s.limits = Limits { memory_bytes: 128 << 20, ..Limits::default() };
    let o = run(&d.sandbox, &s);
    assert_eq!(o.isolation.memory, Enforcement::Cgroup);
    assert_eq!(o.status, ExitStatus::MemoryLimit, "stdout: {}", o.stdout_lossy());
}

#[test]
fn a_fork_bomb_stops_at_pids_max_in_the_cgroup() {
    let Some(d) = delegated("a_fork_bomb_stops_at_pids_max_in_the_cgroup") else { return };
    let w = workdir();
    let pids = 24;
    for cmd in ["forkbomb", "threadbomb"] {
        let mut s = probe_spec(w.path(), &[cmd, "100000"]);
        s.limits = Limits { pids, ..Limits::default() };
        let o = run(&d.sandbox, &s);
        assert_eq!(o.isolation.tasks, Enforcement::Cgroup, "{:?}", o.isolation.notes);
        let line = o.stdout_lossy();
        let n: u64 = line
            .split_whitespace()
            .nth(1)
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("{cmd}: no count in {line:?} ({:?})", o.status));
        assert!(line.contains("stopped_by EAGAIN"), "{cmd}: the kernel refused the surplus: {line}");
        // the sandbox's init and the probe count too
        assert!(n + 2 <= pids && n + 5 >= pids, "{cmd}: {n} created against a limit of {pids}");
    }
    assert!(d.cgroups().is_empty());
}

#[test]
fn a_cpu_hog_is_ended_by_its_cpu_budget_inside_the_cgroup() {
    let Some(d) = delegated("a_cpu_hog_is_ended_by_its_cpu_budget_inside_the_cgroup") else { return };
    let w = workdir();
    let mut s = probe_spec(w.path(), &["spin"]);
    s.limits = Limits { cpu_s: 1, wall_ms: 30_000, ..Limits::default() };
    let started = Instant::now();
    let o = run(&d.sandbox, &s);
    assert_eq!(o.status, ExitStatus::CpuLimit, "{:?}", o.isolation.notes);
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert!(o.cpu_ms.unwrap_or(0) >= 900, "it really burnt its second of CPU: {:?}", o.cpu_ms);
    assert!(d.cgroups().is_empty());
}

#[test]
fn a_base_that_is_not_a_cgroup_directory_is_reported_and_the_limits_fall_back() {
    // needs no delegation: the base is an ordinary directory, so the kernel offers no controllers there
    let base = workdir();
    let sb = NamespaceSandbox::new().with_cgroup_base(base.path());
    let Some(_ns) = namespace("a_base_that_is_not_a_cgroup_directory_is_reported_and_the_limits_fall_back") else {
        return;
    };
    let why = sb.cgroup_availability().unwrap_err();
    assert!(why.contains("no cgroup limit could be applied") && why.contains("subtree_control"), "{why}");
    assert!(std::fs::read_dir(base.path()).unwrap().next().is_none(), "the throw-away cgroup was removed");

    let w = workdir();
    let o = run(&sb, &spec(&["true"], w.path()));
    assert_eq!(o.status, ExitStatus::Exited(0));
    assert_eq!(o.isolation.cgroup, None, "no cgroup is claimed that does not exist");
    assert_ne!(o.isolation.memory, Enforcement::Cgroup);
    assert!(
        o.isolation.notes.iter().any(|n| n.contains("no cgroup v2 limits") && n.contains("subtree_control")),
        "the reason is in the report: {:?}",
        o.isolation.notes
    );
    // and the memory limit is still enforced, by sampling
    let mut s = probe_spec(w.path(), &["alloc", "4000", "1"]);
    s.limits = Limits { memory_bytes: 64 << 20, ..Limits::default() };
    let o = run(&sb, &s);
    assert_eq!(o.status, ExitStatus::MemoryLimit, "stdout: {}", o.stdout_lossy());
    assert!(matches!(o.isolation.memory, Enforcement::Sampled | Enforcement::Rlimit));
}
