//! Best-effort cgroup v2 limits for the namespace sandbox.
//!
//! cgroups give the kernel-exact memory and task limits that `rlimit`s cannot (rlimits are per process).
//! They need a cgroup subtree the running user may write to, with the `memory`/`pids` controllers enabled
//! for it — typically a systemd user slice or a container's own cgroup. When that is not the case (the
//! common situation on a developer laptop, in many CI containers, and on cgroup-v1 hosts) nothing is
//! created and the run reports `cgroup: None`; the rlimits and the sampled memory limit remain in force.
//!
//! The kernel-facing behaviour of this module could **not** be exercised on the development host (it has a
//! cgroup v1 hierarchy plus a v2 mount without the memory or pids controllers). The file handling is covered
//! by unit tests against a fake cgroup directory; see `docs/architecture/sandbox.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::outcome::CgroupReport;
use crate::spec::Limits;

const CGROUP2_SUPER_MAGIC: i64 = 0x6367_7270;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A cgroup created for one run; removed when dropped.
pub(crate) struct Cgroup {
    dir: PathBuf,
    report: CgroupReport,
}

/// The directory under which child cgroups may be created, if there is a usable cgroup v2 hierarchy.
pub(crate) fn delegated_base() -> Option<PathBuf> {
    let own = fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = own.lines().find_map(|l| l.strip_prefix("0::"))?.trim_start_matches('/').to_string();
    // Pure v2 hosts mount it at /sys/fs/cgroup; hybrid hosts at /sys/fs/cgroup/unified.
    for mount in ["/sys/fs/cgroup", "/sys/fs/cgroup/unified"] {
        if is_cgroup2(Path::new(mount)) {
            let base = Path::new(mount).join(&rel);
            return writable(&base).then_some(base);
        }
    }
    None
}

fn is_cgroup2(path: &Path) -> bool {
    crate::sys::filesystem_type(path) == Some(CGROUP2_SUPER_MAGIC)
}

fn writable(path: &Path) -> bool {
    crate::sys::is_writable(path)
}

impl Cgroup {
    /// Create a child cgroup of `base` and set the limits that its controllers allow. `None` if no limit
    /// could be applied (the directory is then removed again).
    pub(crate) fn create(base: &Path, limits: &Limits, task_limit: u64) -> Option<Cgroup> {
        let name = format!("ag-sandbox-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let dir = base.join(name);
        fs::create_dir(&dir).ok()?;
        match Self::configure(dir.clone(), limits, task_limit) {
            Some(cg) => Some(cg),
            None => {
                let _ = fs::remove_dir(&dir);
                None
            }
        }
    }

    /// Apply the limits to an existing cgroup directory.
    pub(crate) fn configure(dir: PathBuf, limits: &Limits, task_limit: u64) -> Option<Cgroup> {
        let controllers = fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        let has = |c: &str| controllers.split_whitespace().any(|w| w == c);
        let mut report = CgroupReport::default();
        if has("memory") {
            report.memory = fs::write(dir.join("memory.max"), limits.memory_bytes.to_string()).is_ok();
            // No swap, and kill the whole group on OOM so that no half-dead process tree lingers.
            let _ = fs::write(dir.join("memory.swap.max"), "0");
            let _ = fs::write(dir.join("memory.oom.group"), "1");
        }
        if has("pids") {
            report.pids = fs::write(dir.join("pids.max"), task_limit.to_string()).is_ok();
        }
        (report.memory || report.pids).then_some(Cgroup { dir, report })
    }

    /// What was achieved.
    pub(crate) fn report(&self) -> CgroupReport {
        self.report
    }

    /// Move a process (and so everything it forks) into the cgroup.
    pub(crate) fn add(&self, pid: libc::pid_t) -> io::Result<()> {
        fs::write(self.dir.join("cgroup.procs"), pid.to_string())
    }

    /// Peak memory use of the group, if the kernel reports it (5.19+).
    pub(crate) fn peak_bytes(&self) -> Option<u64> {
        fs::read_to_string(self.dir.join("memory.peak")).ok()?.trim().parse().ok()
    }

    /// Number of times the kernel OOM-killed in this group.
    pub(crate) fn oom_kills(&self) -> u64 {
        let ev = fs::read_to_string(self.dir.join("memory.events")).unwrap_or_default();
        ev.lines().find_map(|l| l.strip_prefix("oom_kill ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        // Anything left (there should be nothing: the PID namespace is gone) is killed first, then the
        // now-empty directory removed; the kernel refuses to remove a populated cgroup.
        let _ = fs::write(self.dir.join("cgroup.kill"), "1");
        for _ in 0..50 {
            if fs::remove_dir(&self.dir).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(controllers: &str) -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("cg");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("cgroup.controllers"), controllers).unwrap();
        (t, dir)
    }

    #[test]
    fn limits_are_written_when_the_controllers_are_enabled() {
        let (_t, dir) = fake("cpu memory pids\n");
        let limits = Limits { memory_bytes: 123_456_789, ..Limits::default() };
        let cg = Cgroup::configure(dir.clone(), &limits, 77).unwrap();
        assert_eq!(cg.report(), CgroupReport { memory: true, pids: true });
        assert_eq!(fs::read_to_string(dir.join("memory.max")).unwrap(), "123456789");
        assert_eq!(fs::read_to_string(dir.join("pids.max")).unwrap(), "77");
        assert_eq!(fs::read_to_string(dir.join("memory.swap.max")).unwrap(), "0");
        assert_eq!(fs::read_to_string(dir.join("memory.oom.group")).unwrap(), "1");
        cg.add(4242).unwrap();
        assert_eq!(fs::read_to_string(dir.join("cgroup.procs")).unwrap(), "4242");
    }

    #[test]
    fn partial_delegation_is_reported_as_such() {
        let (_t, dir) = fake("pids");
        let cg = Cgroup::configure(dir, &Limits::default(), 10).unwrap();
        assert_eq!(cg.report(), CgroupReport { memory: false, pids: true });
    }

    #[test]
    fn no_usable_controller_means_no_cgroup() {
        let (_t, dir) = fake("cpu io\n");
        assert!(Cgroup::configure(dir, &Limits::default(), 10).is_none());
        let (_t, dir) = fake("");
        assert!(Cgroup::configure(dir, &Limits::default(), 10).is_none());
    }

    #[test]
    fn oom_events_and_peak_are_parsed() {
        let (_t, dir) = fake("memory");
        fs::write(dir.join("memory.events"), "low 0\nhigh 0\nmax 12\noom 3\noom_kill 2\n").unwrap();
        fs::write(dir.join("memory.peak"), "987654\n").unwrap();
        let cg = Cgroup::configure(dir, &Limits::default(), 10).unwrap();
        assert_eq!(cg.oom_kills(), 2);
        assert_eq!(cg.peak_bytes(), Some(987_654));
    }
}
