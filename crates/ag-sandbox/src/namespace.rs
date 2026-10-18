//! The rootless Linux sandbox: user, mount, PID, network, IPC and UTS namespaces, a private root file system,
//! dropped capabilities, `no_new_privs`, a seccomp deny-list and resource limits. No external binaries and no
//! privileges are needed. See `docs/architecture/sandbox.md` for the threat model.

use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use libc::{c_ulong, pid_t};

use crate::cgroup::{self, Cgroup};
use crate::detect::Availability;
use crate::digest::environment_digest;
use crate::error::SandboxError;
use crate::mem::Sampler;
use crate::nsinit::{MountOp, NsPlan};
use crate::outcome::{Enforcement, ExitStatus, IsolationLevel, IsolationReport, Outcome};
use crate::planning::{exec_inputs, path_cstring};
use crate::spawn::{self, Failure, INNER_ID, Plan, Res, stage};
use crate::spec::{Limits, Network, SandboxSpec, WORK_DIR, normalize_sandbox_path};
use crate::supervise::{Hooks, classify, supervise};
use crate::sys;
use crate::{Sandbox, seccomp};

/// Host directories tried, in order, as the place to mount the new root's tmpfs. The mount is only visible
/// inside the sandbox's own mount namespace, and the directory's contents are reachable again afterwards
/// through `/oldroot`, so the choice does not matter as long as it exists.
const BASE_CANDIDATES: [&str; 7] = ["/tmp", "/mnt", "/run", "/var", "/srv", "/opt", "/media"];

/// Device nodes bound into the sandbox's `/dev`.
const DEVICES: [&str; 5] = ["null", "zero", "full", "random", "urandom"];

/// Upper bound for `/dev/shm`, whatever `rw_tmp_bytes` is.
const SHM_MAX: u64 = 64 << 20;

/// Parts of `/proc` made read-only. They control the whole host (sysctls, SysRq, binfmt_misc), not just the
/// sandbox's PID namespace, and a process whose kernel uid is 0 (a root host with the default id mapping)
/// passes the plain permission check on them even without capabilities. The list is the usual container
/// runtimes' one.
const PROC_READONLY: [&str; 5] = ["/proc/bus", "/proc/fs", "/proc/irq", "/proc/sys", "/proc/sysrq-trigger"];

/// Parts of `/proc` hidden altogether (replaced by `/dev/null` or an empty read-only directory): kernel
/// memory, key material, debugging interfaces.
const PROC_MASKED: [&str; 10] = [
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

/// The `/proc` paths of [`PROC_READONLY`] and [`PROC_MASKED`] that exist on this kernel, with whether each
/// is a directory.
fn existing(paths: &[&'static str]) -> Vec<(&'static str, bool)> {
    paths.iter().filter_map(|p| fs::symlink_metadata(p).ok().map(|m| (*p, m.is_dir()))).collect()
}

/// The rootless namespace sandbox. Cheap to construct and `Send + Sync`; every [`run`](Sandbox::run) creates
/// and destroys its own set of namespaces.
///
/// The command runs as user and group `1000` inside its user namespace, which the kernel maps to a single
/// *host* id: by default the user running the sandbox (so files the command writes to `/work` belong to
/// that user), or, when the host process is root, the ids given to [`with_host_ids`](Self::with_host_ids).
#[derive(Clone, Debug, Default)]
pub struct NamespaceSandbox {
    host_ids: Option<(u32, u32)>,
    cgroup_base: Option<PathBuf>,
}

impl NamespaceSandbox {
    /// A namespace sandbox. Constructing one never fails; use [`availability`](Self::availability) (or
    /// [`crate::detect`]) to learn whether this host can run it.
    pub fn new() -> Self {
        NamespaceSandbox { host_ids: None, cgroup_base: None }
    }

    /// Map the command's id to the unprivileged **host** user and group `uid`/`gid` instead of to the user
    /// running the sandbox. Only possible when that user is root (the kernel allows nothing else); `run`
    /// fails otherwise.
    ///
    /// Why a root host should do this: (1) the kernel exempts host uid 0 from `RLIMIT_NPROC`, so as root
    /// the `pids` limit can only be enforced by sampling, whereas for any other uid it is exact;
    /// (2) a command that somehow left its namespaces would be that unprivileged user, not root. The price:
    /// the command can write only what that uid may write, so `workdir` must be owned by (or writable for)
    /// it — chown the materialized tree first.
    #[must_use]
    pub fn with_host_ids(mut self, uid: u32, gid: u32) -> Self {
        self.host_ids = Some((uid, gid));
        self
    }

    /// Create the per-run cgroups (kernel-exact `memory` and `pids` limits) under `base` instead of looking for
    /// a place: a directory the sandbox's user may write to, with the memory and pids controllers enabled in
    /// its `cgroup.subtree_control` and no processes of its own. Without this the environment variable
    /// `AG_SANDBOX_CGROUP_BASE` is consulted, then the process's own cgroup and its parent. See
    /// [`cgroup_availability`](Self::cgroup_availability) to find out whether it works, and
    /// `docs/architecture/sandbox.md` for how to prepare one.
    #[must_use]
    pub fn with_cgroup_base(mut self, base: impl Into<PathBuf>) -> Self {
        self.cgroup_base = Some(base.into());
        self
    }

    /// What cgroup v2 limits a run would get: makes (and removes) a throw-away cgroup the way a run does and
    /// reports which controllers the kernel accepted, or why there is no usable base. A run without cgroups is
    /// not an error (the sampled and rlimit limits stay in force), but an operator who configured a base wants
    /// to know at start-up that it does not work.
    pub fn cgroup_availability(&self) -> Result<crate::CgroupReport, String> {
        let base = cgroup::delegated_base(self.cgroup_base.as_deref())?;
        let cg = Cgroup::create(&base, &Limits::default(), Limits::default().pids)?;
        Ok(cg.report())
    }

    /// Whether unprivileged namespaces, `pivot_root`, a fresh `/proc` and the seccomp filter all work here.
    ///
    /// The answer comes from actually performing the complete setup once (up to, but not including, executing
    /// a command) and is cached for the life of the process.
    pub fn availability(&self) -> Availability {
        static PROBE: OnceLock<Availability> = OnceLock::new();
        PROBE.get_or_init(probe).clone()
    }
}

impl Sandbox for NamespaceSandbox {
    fn name(&self) -> &'static str {
        "namespace"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::Namespaces
    }

    fn run_cancellable(&self, spec: &SandboxSpec, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, SandboxError> {
        spec.validate()?;
        if let Availability::Unavailable(reason) = self.availability() {
            return Err(SandboxError::Unavailable { sandbox: "namespace", reason });
        }
        let (euid, egid) = sys::euid_egid();
        if euid != 0 && self.host_ids.is_some_and(|ids| ids != (euid, egid)) {
            return Err(SandboxError::invalid(
                "with_host_ids can only map to ids other than the caller's when the caller is root",
            ));
        }
        let ids = self.host_ids.unwrap_or((euid, egid));
        let plan = build_plan(spec, false, ids != (euid, egid))?;
        run_plan(spec, plan, ids, self.cgroup_base.as_deref(), cancel)
    }
}

/// Complete a trial setup with a throw-away tmpfs root and no command.
fn probe() -> Availability {
    if !cfg!(target_os = "linux") {
        return Availability::Unavailable("namespaces are a Linux feature".into());
    }
    if !seccomp::supported() {
        return Availability::Unavailable(
            "no seccomp filter is defined for this CPU architecture (x86_64 and aarch64 are supported)".into(),
        );
    }
    let mut spec = SandboxSpec::new(["true"], std::env::temp_dir());
    spec.limits.wall_ms = 10_000;
    let ids = sys::euid_egid();
    let result = build_plan(&spec, true, false).and_then(|plan| run_plan(&spec, plan, ids, None, &|| false));
    match result {
        Ok(_) => Availability::Available,
        Err(e) => Availability::Unavailable(format!("{e}{}", hint())),
    }
}

/// Pointers to the usual reasons unprivileged namespaces are refused.
fn hint() -> String {
    let read = |p: &str| fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    let mut hints = Vec::new();
    if read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        hints.push("sysctl kernel.unprivileged_userns_clone=0 disables unprivileged user namespaces");
    }
    if read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0") {
        hints.push("sysctl user.max_user_namespaces=0 disables user namespaces");
    }
    if read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") {
        hints.push("AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns=1)");
    }
    if hints.is_empty() {
        hints.push(
            "a container runtime's seccomp profile or LSM policy may forbid clone(CLONE_NEWUSER), mount or pivot_root",
        );
    }
    format!(" ({})", hints.join("; "))
}

// --- planning ------------------------------------------------------------------------------------------------

struct OpsBuilder {
    ops: Vec<MountOp>,
    made: BTreeSet<PathBuf>,
}

impl OpsBuilder {
    fn new() -> Self {
        OpsBuilder { ops: Vec::new(), made: BTreeSet::new() }
    }

    /// `mkdir -p` for the sandbox path `p` (parents first, each directory once).
    fn mkdir_p(&mut self, p: &Path) -> Result<(), SandboxError> {
        let mut chain: Vec<&Path> =
            p.ancestors().filter(|a| *a != Path::new("/") && !a.as_os_str().is_empty()).collect();
        chain.reverse();
        for dir in chain {
            if self.made.insert(dir.to_path_buf()) {
                self.ops.push(MountOp::Mkdir(path_cstring(dir)?));
            }
        }
        Ok(())
    }

    fn fs(
        &mut self,
        fstype: &'static std::ffi::CStr,
        target: &Path,
        flags: c_ulong,
        data: Option<String>,
    ) -> Result<(), SandboxError> {
        self.mkdir_p(target)?;
        let data = data.map(|d| CString::new(d).expect("mount data has no NUL"));
        self.ops.push(MountOp::Fs { fstype, target: path_cstring(target)?, flags, data });
        Ok(())
    }

    /// Bind an existing path onto itself (or another existing path) and restrict it; nothing is created.
    fn bind_existing(&mut self, source: &Path, target: &Path, extra: c_ulong) -> Result<(), SandboxError> {
        self.ops.push(MountOp::Bind { source: path_cstring(source)?, target: path_cstring(target)?, extra });
        Ok(())
    }

    /// Bind `source` (a path inside `/oldroot`) on `target`, creating the mount point as a directory or an
    /// empty file to match.
    fn bind(&mut self, source: &Path, is_dir: bool, target: &Path, extra: c_ulong) -> Result<(), SandboxError> {
        if is_dir {
            self.mkdir_p(target)?;
        } else {
            if let Some(parent) = target.parent() {
                self.mkdir_p(parent)?;
            }
            self.ops.push(MountOp::Touch(path_cstring(target)?));
        }
        self.ops.push(MountOp::Bind { source: path_cstring(source)?, target: path_cstring(target)?, extra });
        // Whatever the mount brings along exists now; do not try to create it again.
        self.made.insert(target.to_path_buf());
        Ok(())
    }
}

fn old_root(host: &Path) -> PathBuf {
    let mut p = PathBuf::from("/oldroot");
    p.push(host.strip_prefix("/").unwrap_or(host));
    p
}

fn tmpfs_options(bytes: u64, mode: &str) -> String {
    // A page-sized inode budget keeps a flood of tiny files from exhausting kernel memory beyond the byte cap.
    let inodes = (bytes / 4096).clamp(1024, 1 << 20);
    format!("mode={mode},size={bytes},nr_inodes={inodes}")
}

fn build_plan(spec: &SandboxSpec, probe: bool, switch_ids: bool) -> Result<Plan, SandboxError> {
    let seccomp = seccomp::build()?;

    let base = BASE_CANDIDATES
        .iter()
        .find(|p| fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false))
        .ok_or_else(|| SandboxError::Unavailable {
            sandbox: "namespace",
            reason: format!("none of {BASE_CANDIDATES:?} exists as a real directory to build the sandbox root on"),
        })?;

    let mut b = OpsBuilder::new();
    let (nosuid, nodev, noexec, rdonly) =
        (libc::MS_NOSUID as c_ulong, libc::MS_NODEV as c_ulong, libc::MS_NOEXEC as c_ulong, libc::MS_RDONLY as c_ulong);

    // /proc of the new PID namespace. Mounted before the old root is dropped, because the kernel only lets an
    // unprivileged mount namespace mount procfs if a fully visible one exists in it.
    b.fs(c"proc", Path::new("/proc"), nosuid | nodev | noexec, None)?;

    // A minimal /dev: five device nodes bound from the host, the usual fd symlinks and a small /dev/shm.
    b.fs(c"tmpfs", Path::new("/dev"), nosuid | noexec, Some("mode=0755,size=64k,nr_inodes=64".into()))?;
    for dev in DEVICES {
        let source = old_root(&Path::new("/dev").join(dev));
        b.bind(&source, false, &Path::new("/dev").join(dev), nosuid | noexec)?;
    }
    for (link, target) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        b.ops.push(MountOp::Symlink {
            target: path_cstring(Path::new(target))?,
            link: path_cstring(&Path::new("/dev").join(link))?,
        });
    }
    b.fs(
        c"tmpfs",
        Path::new("/dev/shm"),
        nosuid | nodev | noexec,
        Some(tmpfs_options(spec.rw_tmp_bytes.min(SHM_MAX), "1777")),
    )?;

    // Harden /proc now that /dev/null exists: what is shared with the host stays out of reach.
    for (path, _) in existing(&PROC_READONLY) {
        b.bind_existing(Path::new(path), Path::new(path), rdonly | nosuid | nodev)?;
    }
    for (path, is_dir) in existing(&PROC_MASKED) {
        if is_dir {
            let flags = rdonly | nosuid | nodev | noexec;
            b.ops.push(MountOp::Fs {
                fstype: c"tmpfs",
                target: path_cstring(Path::new(path))?,
                flags,
                data: Some(c"mode=0555,size=4k,nr_inodes=2".to_owned()),
            });
        } else {
            // (no `nodev`: opening a device node through a nodev mount fails, and /dev/null is a device node)
            b.bind_existing(Path::new("/dev/null"), Path::new(path), rdonly | nosuid | noexec)?;
        }
    }

    // The working tree, read-write, and a size-capped /tmp.
    let workdir = fs::canonicalize(&spec.workdir)
        .map_err(|e| SandboxError::invalid(format!("workdir {}: {e}", spec.workdir.display())))?;
    b.bind(&old_root(&workdir), true, Path::new(WORK_DIR), nosuid | nodev)?;
    b.fs(c"tmpfs", Path::new("/tmp"), nosuid | nodev, Some(tmpfs_options(spec.rw_tmp_bytes, "1777")))?;

    // Read-only mounts, shallowest first so that a mount can be nested inside an earlier one.
    let mut mounts: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    for m in &spec.ro_mounts {
        let host = fs::canonicalize(&m.host)
            .map_err(|e| SandboxError::invalid(format!("mount source {}: {e}", m.host.display())))?;
        let is_dir = fs::metadata(&host).map(|md| md.is_dir()).unwrap_or(true);
        mounts.push((normalize_sandbox_path(&m.sandbox)?, host, is_dir));
    }
    mounts.sort();
    for (target, host, is_dir) in &mounts {
        b.bind(&old_root(host), *is_dir, target, rdonly | nosuid | nodev)?;
    }

    let (argv, envp, exec_paths) = exec_inputs(spec)?;
    let l = &spec.limits;
    // The kernel's per-process memory limit is a backstop above the nominal limit, which the supervisor
    // enforces by sampling the whole sandbox; see the docs.
    let data_limit = l.memory_bytes.saturating_add(l.memory_bytes / 4).saturating_add(32 << 20);
    let rlimits = vec![
        (Res::Cpu, l.cpu_s, l.cpu_s.saturating_add(1)),
        (Res::Data, data_limit, data_limit),
        // The sandbox's init is a task of the same user and is counted, so allow one more than the command.
        (Res::Nproc, l.pids.saturating_add(1), l.pids.saturating_add(1)),
        (Res::Nofile, l.open_files, l.open_files),
        (Res::Fsize, l.file_size_bytes, l.file_size_bytes),
        (Res::Core, 0, 0),
    ];

    let mut clone_flags =
        (libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS)
            as c_ulong;
    if spec.network == Network::None {
        clone_flags |= libc::CLONE_NEWNET as c_ulong;
    }

    Ok(Plan {
        clone_flags,
        ns: Some(NsPlan {
            switch_ids: switch_ids.then_some(INNER_ID),
            bring_up_loopback: spec.network == Network::None,
            hostname: c"sandbox".to_owned(),
            base: path_cstring(Path::new(base))?,
            put_old: path_cstring(&Path::new(base).join("oldroot"))?,
            root_opts: c"mode=0755,size=4m,nr_inodes=4096".to_owned(),
            ops: b.ops,
        }),
        argv,
        envp,
        exec_paths,
        cwd: CString::new(WORK_DIR).expect("no NUL"),
        rlimits,
        seccomp,
        no_new_privs: true,
        probe_only: probe,
    })
}

// --- running -------------------------------------------------------------------------------------------------

/// Let the new user namespace know who it is: one id, `INNER_ID`, mapped to host ids `(uid, gid)`.
fn write_id_maps(pid: pid_t, (uid, gid): (u32, u32)) -> Result<(), SandboxError> {
    let (euid, _) = sys::euid_egid();
    let dir = format!("/proc/{pid}");
    let w = |file: &str, content: String| {
        fs::write(format!("{dir}/{file}"), content).map_err(|e| SandboxError::setup(format!("write {file}"), e))
    };
    // An unprivileged process may write a one-line map for itself only after giving up setgroups(2)
    // (otherwise it could drop a group that denies it access). A privileged one need not.
    if euid != 0 {
        w("setgroups", "deny".into())?;
    }
    w("uid_map", format!("{INNER_ID} {uid} 1\n"))?;
    match w("gid_map", format!("{INNER_ID} {gid} 1\n")) {
        Ok(()) => Ok(()),
        Err(SandboxError::Setup { source, .. }) if euid == 0 && source.kind() == io::ErrorKind::PermissionDenied => {
            // Root without CAP_SETGID (inside another container): fall back to the unprivileged protocol.
            w("setgroups", "deny".into())?;
            w("gid_map", format!("{INNER_ID} {gid} 1\n"))
        }
        Err(e) => Err(e),
    }
}

fn spawn_error(e: io::Error) -> SandboxError {
    match e.raw_os_error() {
        Some(libc::EPERM | libc::EACCES | libc::EINVAL | libc::ENOSPC | libc::EUSERS) => SandboxError::Unavailable {
            sandbox: "namespace",
            reason: format!("clone with new namespaces was refused: {e}{}", hint()),
        },
        _ => SandboxError::setup("create the sandbox process", e),
    }
}

fn failure_error(f: Failure, plan: &Plan, spec: &SandboxSpec) -> SandboxError {
    let source = io::Error::from_raw_os_error(f.errno);
    if f.stage == stage::EXEC {
        SandboxError::Exec { program: spec.argv[0].clone(), source }
    } else {
        SandboxError::setup(f.stage_name(plan), source)
    }
}

fn run_plan(
    spec: &SandboxSpec,
    plan: Plan,
    host_ids: (u32, u32),
    cgroup_base: Option<&Path>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, SandboxError> {
    let started = Instant::now();
    let probe = plan.probe_only;

    // Optional cgroup v2 limits. Never for the availability probe. Why there are none goes into the notes.
    let mut cgroup_note = None;
    let mut cg = if probe {
        None
    } else {
        match cgroup::delegated_base(cgroup_base).and_then(|base| Cgroup::create(&base, &spec.limits, spec.limits.pids))
        {
            Ok(cg) => Some(cg),
            Err(why) => {
                cgroup_note = Some(why);
                None
            }
        }
    };

    let mut child = spawn::spawn(&plan).map_err(spawn_error)?;
    write_id_maps(child.pid, host_ids)?;
    if let Some(Err(e)) = cg.as_ref().map(|c| c.add(child.pid)) {
        cgroup_note = Some(format!("moving the sandbox into its cgroup failed: {e}"));
        cg = None;
    }
    let cgroup_report = cg.as_ref().map(Cgroup::report);
    let cgroup_memory = cgroup_report.is_some_and(|r| r.memory);
    let cgroup_pids = cgroup_report.is_some_and(|r| r.pids);
    // The kernel exempts host uid 0 from RLIMIT_NPROC; for every other uid the limit is exact.
    let kernel_task_limit = host_ids.0 != 0;
    let sample_tasks = !cgroup_pids && !kernel_task_limit;

    // Can the supervisor read the sandbox's /proc? Checked now, while the init is still parked, so that the
    // report does not depend on how long the command happens to run.
    let sampler = Sampler::new(child.pid);
    let readable = !probe && sampler.readable();
    let sample_memory = !cgroup_memory && readable;

    child.release().map_err(|e| SandboxError::setup("release the sandbox", e))?;
    let io = child.take_io();
    let mut sample = || sampler.sample();
    let hooks = Hooks {
        group: true,
        on_kill: None,
        sampler: if (sample_memory || (sample_tasks && readable)) && !probe { Some(&mut sample) } else { None },
        task_limit: if sample_tasks && readable { Some(spec.limits.pids) } else { None },
        cancel: Some(cancel),
    };
    let finished = supervise(child.pid, child.pidfd.as_ref(), io, &spec.stdin, &spec.limits, started, hooks)?;
    child.mark_reaped();
    let relayed = child.relayed_status();

    if let Some(f) = finished.failure {
        return Err(failure_error(f, &plan, spec));
    }
    if relayed.is_none() && finished.verdict.is_none() {
        return Err(SandboxError::setup(
            "sandbox init",
            io::Error::other(format!(
                "exited unexpectedly (wait status {:#x}); stderr: {}",
                finished.reaped.status,
                String::from_utf8_lossy(&finished.stderr)
            )),
        ));
    }
    if probe {
        return if relayed == Some((1, 0)) {
            Ok(empty_outcome())
        } else {
            Err(SandboxError::setup("probe", io::Error::other(format!("the probe command ended with {relayed:?}"))))
        };
    }

    let mut status = classify(&finished, relayed, &spec.limits);
    if finished.verdict.is_none() && cg.as_ref().is_some_and(|c| c.oom_kills() > 0) {
        status = ExitStatus::MemoryLimit;
    }
    let (cpu_ms, _) = sys::usage(&finished.reaped.rusage);
    let max_rss_bytes = cg.as_ref().and_then(Cgroup::peak_bytes).or(finished.peak_sampled);

    let memory = if cgroup_memory {
        Enforcement::Cgroup
    } else if readable {
        Enforcement::Sampled
    } else {
        Enforcement::Rlimit
    };
    let tasks = if cgroup_pids {
        Enforcement::Cgroup
    } else if kernel_task_limit {
        Enforcement::Rlimit
    } else if readable {
        Enforcement::Sampled
    } else {
        Enforcement::None
    };
    let mut notes = Vec::new();
    if memory == Enforcement::Sampled {
        notes.push(
            "memory is limited by sampling the sandbox's anonymous and shared resident memory every 1-10 ms \
             (RLIMIT_DATA at 1.25x + 32 MiB is the per-process backstop); a cgroup would be exact"
                .to_string(),
        );
    }
    if host_ids.0 == 0 {
        notes.push(
            "the command's kernel uid is 0 (the host process is root and no other host ids were chosen): file ownership checks \
             treat it as the owner of every root-owned file it can reach, so /proc is hardened and capabilities dropped, but an \
             escape from the namespaces would be an escape as root; prefer NamespaceSandbox::with_host_ids"
                .into(),
        );
    }
    if tasks == Enforcement::Sampled {
        notes.push(
            "host uid 0 is exempt from RLIMIT_NPROC, so `pids` is enforced by sampling and a fork bomb can briefly \
             exceed it; run as another user or use NamespaceSandbox::with_host_ids"
                .into(),
        );
    }
    if cgroup_report.is_none() {
        notes.push(format!(
            "no cgroup v2 limits: {}",
            cgroup_note.as_deref().unwrap_or("no writable cgroup v2 subtree with the memory/pids controllers")
        ));
    }
    if sys::euid_egid().0 != 0 {
        notes.push(
            "supplementary groups of the host user cannot be dropped without privilege (they cannot grant access \
             to anything outside the mounts)"
                .into(),
        );
    }
    let isolation = IsolationReport {
        verified: true,
        user_ns: true,
        mount_ns: true,
        pid_ns: true,
        net_ns: spec.network == Network::None,
        ipc_ns: true,
        uts_ns: true,
        private_root: true,
        read_only_root: true,
        no_new_privs: true,
        capabilities_dropped: true,
        seccomp: true,
        rlimits: true,
        memory,
        tasks,
        host_root: host_ids.0 == 0,
        cgroup: cgroup_report,
        network: spec.network,
        image: None,
        runtime: None,
        notes,
    };
    Ok(Outcome {
        status,
        stdout: finished.stdout,
        stderr: finished.stderr,
        stdout_truncated: finished.stdout_truncated,
        stderr_truncated: finished.stderr_truncated,
        wall_ms: finished.wall.as_millis() as u64,
        cpu_ms: Some(cpu_ms),
        max_rss_bytes,
        environment_digest: environment_digest("namespace", &isolation, spec),
        isolation,
    })
}

fn empty_outcome() -> Outcome {
    Outcome {
        status: ExitStatus::Exited(0),
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        wall_ms: 0,
        cpu_ms: None,
        max_rss_bytes: None,
        isolation: IsolationReport::default(),
        environment_digest: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_mounts_what_the_spec_asks_for_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let spec = SandboxSpec::new(["sh"], dir.path()).ro_mounts([crate::Mount::new("/usr", "/opt/tools/usr")]);
        let plan = build_plan(&spec, false, false).unwrap();
        let ns = plan.ns.as_ref().unwrap();
        let described: Vec<String> = ns.ops.iter().map(|o| format!("{o:?}")).collect();
        let joined = described.join("\n");
        for must in
            ["\"/proc\"", "\"/dev\"", "\"/dev/null\"", "\"/dev/shm\"", "\"/work\"", "\"/tmp\"", "\"/opt/tools/usr\""]
        {
            assert!(joined.contains(must), "{must} missing from\n{joined}");
        }
        // parents are created before children, and nothing is created twice
        let mkdirs: Vec<&String> = described.iter().filter(|d| d.starts_with("Mkdir")).collect();
        let unique: BTreeSet<_> = mkdirs.iter().collect();
        assert_eq!(mkdirs.len(), unique.len(), "{joined}");
        let pos = |needle: &str| described.iter().position(|d| d.contains(needle)).unwrap();
        assert!(
            pos("Mkdir(\"/opt\")") < pos("Mkdir(\"/opt/tools\")")
                && pos("Mkdir(\"/opt/tools\")") < pos("Mkdir(\"/opt/tools/usr\")")
        );
        // the old root is only referenced through /oldroot
        assert!(joined.contains("/oldroot"));
        assert!(plan.clone_flags & libc::CLONE_NEWNET as c_ulong != 0);
        let host = spec.clone().network(Network::Host);
        let plan = build_plan(&host, false, false).unwrap();
        assert_eq!(plan.clone_flags & libc::CLONE_NEWNET as c_ulong, 0);
        assert!(!plan.ns.unwrap().bring_up_loopback);
    }

    #[test]
    fn limits_become_rlimits() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = SandboxSpec::new(["sh"], dir.path());
        spec.limits.pids = 10;
        spec.limits.memory_bytes = 100 << 20;
        let plan = build_plan(&spec, false, false).unwrap();
        let get = |r: &str| plan.rlimits.iter().find(|(res, ..)| format!("{res:?}") == r).copied().unwrap();
        assert_eq!(get("Nproc").1, 11, "one more than the command may have: the init is counted too");
        assert!(get("Data").1 > spec.limits.memory_bytes);
        assert_eq!((get("Core").1, get("Core").2), (0, 0));
        assert_eq!(get("Cpu").2, get("Cpu").1 + 1);
    }

    #[test]
    fn a_missing_workdir_or_mount_is_rejected_before_anything_runs() {
        let spec = SandboxSpec::new(["sh"], "/definitely/missing");
        assert!(matches!(NamespaceSandbox::new().run(&spec), Err(SandboxError::InvalidSpec(_))));
    }
}
