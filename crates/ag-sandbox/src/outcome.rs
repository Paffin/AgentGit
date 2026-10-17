use serde::{Deserialize, Serialize};

use crate::spec::Network;

/// How a sandboxed command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExitStatus {
    /// The main process exited by itself with this code.
    Exited(i32),
    /// The main process was killed by this signal (and the sandbox did not do it for a reason below).
    Signaled(i32),
    /// The wall-clock budget ran out; the sandbox killed everything.
    TimedOut,
    /// stdout or stderr exceeded `output_bytes`; the sandbox killed everything.
    OutputLimit,
    /// Memory exceeded `memory_bytes`. Reported when the supervisor killed the sandbox for it, when the
    /// runtime's OOM killer fired, or — best effort — when the peak resident set of the run reached the
    /// limit (see `docs/architecture/sandbox.md`, "Resource limits").
    MemoryLimit,
    /// A process used up its CPU budget (`cpu_s`) and was killed by the kernel.
    CpuLimit,
    /// The command had more tasks (processes and threads) than `pids` allows and the supervisor killed
    /// everything. Only reported where the limit is enforced by sampling (see [`Enforcement::Sampled`]);
    /// where the kernel enforces it (`RLIMIT_NPROC`, a pids cgroup, a container runtime) the surplus `fork`
    /// simply fails with `EAGAIN` inside the sandbox and the command ends however it ends.
    PidsLimit,
}

impl ExitStatus {
    /// `true` only for a clean `Exited(0)`.
    pub fn success(&self) -> bool {
        matches!(self, ExitStatus::Exited(0))
    }
}

/// How strongly a sandbox separates the command from the host. Ordered: later variants are stronger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IsolationLevel {
    /// Resource limits and a scrubbed environment only: **not isolation**. The command has the file and
    /// network access of the user running the sandbox.
    None,
    /// Unprivileged kernel namespaces, a private filesystem view, dropped capabilities and a seccomp
    /// filter, all sharing the host kernel.
    Namespaces,
    /// Delegated to a container runtime (`docker`/`podman`) with its own namespaces, capability set,
    /// seccomp profile and cgroups. Still a shared kernel; stronger layering than [`Self::Namespaces`], and
    /// only as strong as the runtime's configuration on the host.
    Container,
}

/// How a resource limit is enforced for one run.
///
/// The same limit can be backed by very different guarantees, and an attestation should say which.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Enforcement {
    /// Not enforced.
    #[default]
    None,
    /// Per-process kernel limits (`setrlimit`). Exact for what they measure, but per process (a memory limit
    /// does not add processes up), and `RLIMIT_NPROC` is not applied to root.
    Rlimit,
    /// Measured by the supervisor every few milliseconds over the whole sandbox, which is killed when the
    /// limit is exceeded. Bounds the total, but a burst between two samples can overshoot.
    Sampled,
    /// A cgroup v2 limit: exact, enforced by the kernel for the whole sandbox.
    Cgroup,
    /// Delegated to the container runtime (requested, not observed).
    Runtime,
}

/// What a cgroup v2 limit set achieved (best effort).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CgroupReport {
    /// `memory.max` was set (exact memory limit, kernel OOM kill).
    pub memory: bool,
    /// `pids.max` was set (exact task limit).
    pub pids: bool,
}

/// Which isolation mechanisms were *actually* in force for one run.
///
/// For [`crate::NamespaceSandbox`] and [`crate::ProcessSandbox`] each flag is `true` only if the step was
/// performed successfully (a required step that fails aborts the run instead of being skipped), and the
/// integration tests read the kernel's own view from inside to confirm it. For [`crate::OciSandbox`] the
/// flags describe what was *requested* from the runtime ([`verified`](Self::verified) is `false`).
///
/// The report feeds [`crate::environment_digest`]; attestations bound to that digest therefore state which
/// protections their check ran under.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IsolationReport {
    /// `true` if this crate set the mechanisms up and could observe the result; `false` if they were only
    /// requested from an external runtime.
    pub verified: bool,
    /// New user namespace (the command is not the host user, and has no host capabilities).
    pub user_ns: bool,
    /// New mount namespace.
    pub mount_ns: bool,
    /// New PID namespace (the host's processes are invisible and unsignalable; the sandbox's init dying
    /// kills everything).
    pub pid_ns: bool,
    /// New network namespace. With [`Network::None`] it contains only a loopback interface.
    pub net_ns: bool,
    /// New IPC namespace (System V IPC, POSIX queues).
    pub ipc_ns: bool,
    /// New UTS namespace (host name).
    pub uts_ns: bool,
    /// The root file system was replaced (`pivot_root` into a fresh tmpfs, or the container image); only the
    /// mounts listed in the spec are visible.
    pub private_root: bool,
    /// The root file system is read-only; only `/work`, `/tmp` and `/dev/shm` are writable.
    pub read_only_root: bool,
    /// `PR_SET_NO_NEW_PRIVS` is set: `execve` can never raise privileges (set-user-ID, file capabilities).
    pub no_new_privs: bool,
    /// All capabilities were dropped, including from the bounding set.
    pub capabilities_dropped: bool,
    /// A seccomp-bpf filter is installed (a deny-list for [`crate::NamespaceSandbox`]; the runtime's
    /// default profile for OCI).
    pub seccomp: bool,
    /// `setrlimit` limits (CPU, data, processes, files, file size, core) were applied.
    pub rlimits: bool,
    /// The command's *kernel* uid is 0: the host process is root and the id map points at it. The command is
    /// still unprivileged inside its namespaces, but anything that checks file ownership by kernel id treats
    /// it as root, the kernel does not apply `RLIMIT_NPROC` to it, and a hypothetical escape would be one as
    /// root. `false` when the command maps to an unprivileged host user.
    pub host_root: bool,
    /// How `memory_bytes` is enforced.
    pub memory: Enforcement,
    /// How `pids` is enforced.
    pub tasks: Enforcement,
    /// cgroup v2 limits, if a delegated cgroup was writable. `None` means no cgroup was used.
    pub cgroup: Option<CgroupReport>,
    /// The network mode the command ran with.
    pub network: Network,
    /// The container image (OCI only), as given — digest-pinned references are preferred.
    pub image: Option<String>,
    /// The container runtime executable (OCI only).
    pub runtime: Option<String>,
    /// Plain-language remarks about best-effort or missing mechanisms. Informational; not part of the digest.
    pub notes: Vec<String>,
}

/// The result of one run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Outcome {
    /// How the command ended.
    pub status: ExitStatus,
    /// Captured standard output (at most `output_bytes`).
    pub stdout: Vec<u8>,
    /// Captured standard error (at most `output_bytes`).
    pub stderr: Vec<u8>,
    /// `stdout` was cut at `output_bytes`.
    pub stdout_truncated: bool,
    /// `stderr` was cut at `output_bytes`.
    pub stderr_truncated: bool,
    /// Wall-clock milliseconds from sandbox creation until everything was gone.
    pub wall_ms: u64,
    /// User + system CPU milliseconds of the main process and the descendants it waited for, if known.
    pub cpu_ms: Option<u64>,
    /// Peak resident set of the largest single process, if known (not the sum over processes).
    pub max_rss_bytes: Option<u64>,
    /// The mechanisms that were in force.
    pub isolation: IsolationReport,
    /// [`crate::environment_digest`] of this run's environment, as `sha256:<hex>`. Bind attestations to it.
    pub environment_digest: String,
}

impl Outcome {
    /// `true` if the command exited with status 0.
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// Standard output as text (lossy UTF-8).
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Standard error as text (lossy UTF-8).
    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}
