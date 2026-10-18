//! cgroup v2 limits for the namespace sandbox.
//!
//! cgroups give the kernel-exact memory and task limits that `rlimit`s cannot (rlimits are per process).
//! They need a cgroup directory the running user may write to, with the `memory` / `pids` controllers enabled
//! for its children (`cgroup.subtree_control`): a systemd unit with `Delegate=` and `DelegateSubgroup=`, a
//! container's own cgroup, or a directory an administrator prepared (see below). When none is available (the
//! common situation on a developer laptop and on cgroup-v1 hosts) nothing is created and the run reports
//! `cgroup: None` with the reason in its notes; the rlimits and the sampled memory limit remain in force.
//!
//! Where the child cgroups are made:
//!
//! 1. the base given with [`crate::NamespaceSandbox::with_cgroup_base`], else
//! 2. the directory named by the `AG_SANDBOX_CGROUP_BASE` environment variable, else
//! 3. the process's own cgroup, or its parent (the layout of a unit started with `DelegateSubgroup=`), if one of
//!    them is writable and has the controllers enabled.
//!
//! A cgroup that has controllers enabled for its children cannot also hold processes (the kernel's "no
//! internal processes" rule), so an explicit base must be an *empty* directory next to the process, not the
//! directory the process lives in: for example `mkdir /sys/fs/cgroup/<unit>/sandboxes` and
//! `echo "+memory +pids" > /sys/fs/cgroup/<unit>/sandboxes/cgroup.subtree_control`, owned by the worker's user.
//!
//! The kernel-facing behaviour is exercised by `tests/cgroup_real.rs` (and the rest of the namespace suites run
//! inside the same delegation) in the `OCI runtimes and cgroup v2` workflow; the file handling is also covered
//! by unit tests against a fake directory. `docs/architecture/sandbox.md` says which is verified where.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::outcome::CgroupReport;
use crate::spec::Limits;

const CGROUP2_SUPER_MAGIC: i64 = 0x6367_7270;

/// The environment variable that names the directory under which the namespace sandbox creates its cgroups.
pub const CGROUP_BASE_ENV: &str = "AG_SANDBOX_CGROUP_BASE";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A cgroup created for one run; removed when dropped.
pub(crate) struct Cgroup {
    dir: PathBuf,
    report: CgroupReport,
}

/// The directory under which child cgroups are to be created, if one is configured or can be found.
pub(crate) fn delegated_base(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os(CGROUP_BASE_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let own = fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("cannot read /proc/self/cgroup: {e}"))?;
    let rel = own
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("this process is not in a cgroup v2 hierarchy (cgroup v1 host)")?
        .trim_start_matches('/')
        .to_string();
    // Pure v2 hosts mount it at /sys/fs/cgroup; hybrid hosts at /sys/fs/cgroup/unified.
    let mount = ["/sys/fs/cgroup", "/sys/fs/cgroup/unified"]
        .into_iter()
        .map(Path::new)
        .find(|m| is_cgroup2(m))
        .ok_or("no cgroup v2 file system is mounted at /sys/fs/cgroup or /sys/fs/cgroup/unified")?;
    let own_dir = mount.join(&rel);
    let mut candidates = vec![own_dir.clone()];
    if let Some(parent) = own_dir.parent().filter(|p| p.starts_with(mount) && *p != mount) {
        candidates.push(parent.to_path_buf());
    }
    // The root cgroup is a candidate only as the process's own (a process in it may enable controllers there).
    let mut why = Vec::new();
    for dir in &candidates {
        match usable(dir) {
            Ok(()) => return Ok(dir.clone()),
            Err(e) => why.push(e),
        }
    }
    Err(why.join("; "))
}

/// Whether `dir` can serve as a base: writable, and the controllers enabled for its children.
fn usable(dir: &Path) -> Result<(), String> {
    if !crate::sys::is_writable(dir) {
        return Err(format!("{} is not writable by this user", dir.display()));
    }
    let enabled = fs::read_to_string(dir.join("cgroup.subtree_control")).unwrap_or_default();
    let has = |c: &str| enabled.split_whitespace().any(|w| w == c);
    if has("memory") || has("pids") {
        Ok(())
    } else {
        Err(format!("{} has no memory or pids controller enabled in its cgroup.subtree_control", dir.display()))
    }
}

fn is_cgroup2(path: &Path) -> bool {
    crate::sys::filesystem_type(path) == Some(CGROUP2_SUPER_MAGIC)
}

impl Cgroup {
    /// Create a child cgroup of `base` and set the limits that its controllers allow. `Err` with the reason if
    /// no limit could be applied (the directory is then removed again).
    pub(crate) fn create(base: &Path, limits: &Limits, task_limit: u64) -> Result<Cgroup, String> {
        let name = format!("ag-sandbox-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let dir = base.join(name);
        fs::create_dir(&dir).map_err(|e| format!("cannot create a cgroup in {}: {e}", base.display()))?;
        Self::configure(dir.clone(), limits, task_limit).inspect_err(|_| {
            let _ = fs::remove_dir(&dir);
        })
    }

    /// Apply the limits to an existing cgroup directory.
    pub(crate) fn configure(dir: PathBuf, limits: &Limits, task_limit: u64) -> Result<Cgroup, String> {
        let controllers = fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        let has = |c: &str| controllers.split_whitespace().any(|w| w == c);
        let mut report = CgroupReport::default();
        let mut why = Vec::new();
        if has("memory") {
            match fs::write(dir.join("memory.max"), limits.memory_bytes.to_string()) {
                Ok(()) => report.memory = true,
                Err(e) => why.push(format!("memory.max: {e}")),
            }
            // No swap, and kill the whole group on OOM so that no half-dead process tree lingers.
            let _ = fs::write(dir.join("memory.swap.max"), "0");
            let _ = fs::write(dir.join("memory.oom.group"), "1");
        } else {
            why.push("the memory controller is not enabled for it".to_string());
        }
        if has("pids") {
            match fs::write(dir.join("pids.max"), task_limit.to_string()) {
                Ok(()) => report.pids = true,
                Err(e) => why.push(format!("pids.max: {e}")),
            }
        } else {
            why.push("the pids controller is not enabled for it".to_string());
        }
        if report.memory || report.pids {
            Ok(Cgroup { dir, report })
        } else {
            Err(format!(
                "no cgroup limit could be applied in {} (controllers there: [{}]): {}; enable them in the parent's cgroup.subtree_control",
                dir.display(),
                controllers.trim(),
                why.join(", ")
            ))
        }
    }

    /// What was achieved.
    pub(crate) fn report(&self) -> CgroupReport {
        self.report
    }

    /// The directory of this cgroup.
    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
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
    fn no_usable_controller_means_no_cgroup_and_says_why() {
        let (_t, dir) = fake("cpu io\n");
        let why = Cgroup::configure(dir, &Limits::default(), 10).err().unwrap();
        assert!(why.contains("memory controller is not enabled") && why.contains("subtree_control"), "{why}");
        let (_t, dir) = fake("");
        assert!(Cgroup::configure(dir, &Limits::default(), 10).is_err());
    }

    #[test]
    fn oom_events_and_peak_are_parsed() {
        let (_t, dir) = fake("memory");
        fs::write(dir.join("memory.events"), "low 0\nhigh 0\nmax 12\noom 3\noom_kill 2\n").unwrap();
        fs::write(dir.join("memory.peak"), "987654\n").unwrap();
        let cg = Cgroup::configure(dir, &Limits::default(), 10).unwrap();
        assert_eq!(cg.oom_kills(), 2);
        assert_eq!(cg.peak_bytes(), Some(987_654));
        assert!(cg.dir().ends_with("cg"));
    }

    #[test]
    fn an_explicit_base_wins_over_everything_else() {
        assert_eq!(delegated_base(Some(Path::new("/some/where"))).unwrap(), Path::new("/some/where"));
    }

    #[test]
    fn a_base_needs_to_be_writable_and_to_have_controllers_enabled_for_its_children() {
        let t = tempfile::tempdir().unwrap();
        let why = usable(t.path()).unwrap_err();
        assert!(why.contains("no memory or pids controller"), "{why}");
        fs::write(t.path().join("cgroup.subtree_control"), "cpu memory pids\n").unwrap();
        assert!(usable(t.path()).is_ok());
        assert!(usable(&t.path().join("missing")).unwrap_err().contains("not writable"));
    }
}
