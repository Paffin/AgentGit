//! Container isolation through a `docker` or `podman` executable.
//!
//! This sandbox builds the `run` command line and supervises the runtime client; the isolation itself is the
//! runtime's. It is the stronger option for hostile workloads (separate runtime-managed namespaces, a
//! default seccomp *allow-list*, AppArmor/SELinux where configured, cgroup limits enforced by the kernel),
//! at the price of a daemon (Docker) or a rootless-container setup (Podman) and a prepared image.
//!
//! What is verified, and where:
//!
//! * `tests/oci_real.rs` runs the sandbox against a **real** Docker and a **real** Podman (the `OCI runtimes`
//!   workflow) and checks from *inside* the container what the flags are supposed to achieve: no network,
//!   read-only root, `NoNewPrivs`, no capabilities, memory / pids / CPU limits, no visibility of the worker's
//!   keys and credentials, cleanup. A flag a runtime rejects or silently drops is a failure there.
//! * On every run the runtime's own record of the container (`inspect`) is compared with the request
//!   (`oci_record`): a protection the runtime recorded differently makes the run fail with
//!   [`SandboxError::Runtime`] instead of producing a verdict, and the report claims only what was read back.
//! * `tests/oci.rs` runs a fake runtime script and covers only argument construction and supervision (timeouts,
//!   output caps, stdin, cleanup ordering); it proves nothing about a real runtime.

use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::digest::environment_digest;
use crate::error::SandboxError;
use crate::oci_record::{self, Check, DaemonFacts, Observed, Requested, State};
use crate::outcome::{CgroupReport, Enforcement, ExitStatus, IsolationLevel, IsolationReport, Outcome};
use crate::spec::{Network, SandboxSpec, WORK_DIR, normalize_sandbox_path};
use crate::supervise::{Finished, Hooks, Verdict, supervise, wait_status};
use crate::{Sandbox, sys};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The label every container of this sandbox carries, so that leftovers can be found.
const LABEL: &str = "ag-sandbox=1";
/// The prefix of the label that names the host a container was started from.
const HOST_LABEL: &str = "ag-sandbox.host";
/// The prefix of every container name; the process id of the starting process follows.
const NAME_PREFIX: &str = "ag-sandbox-";

/// Environment variables the runtime client itself needs to find its daemon or rootless setup. The
/// command's environment never travels this way (it goes through a private `--env-file`).
const CLIENT_ENV: [&str; 16] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "CONTAINER_HOST",
    "CONTAINERS_CONF",
];

/// The default for [`OciConfig::control_timeout`].
pub const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// The default for [`OciConfig::remove_timeout`].
pub const DEFAULT_REMOVE_TIMEOUT: Duration = Duration::from_secs(20);

/// The most of a housekeeping command's output that is read (`inspect` and `info` are tens of kilobytes).
const CAPTURE_LIMIT: u64 = 8 << 20;

/// How to reach the container runtime and which image to run in.
#[derive(Clone, Debug, PartialEq)]
pub struct OciConfig {
    /// `docker`, `podman`, or a path to a compatible executable.
    pub runtime: String,
    /// The image reference. Must be pinned by digest (`name@sha256:<64 hex>`) unless
    /// [`allow_unpinned_image`](Self::allow_unpinned_image) is set, so that "the environment a check ran in"
    /// names one exact image. The image must already be in the runtime's local store: the sandbox never
    /// pulls (`--pull never`), so that what runs is what the operator put there.
    pub image: String,
    /// `--user` value; the default is the conventional unprivileged `65534:65534`. The user must be able to
    /// write the bind-mounted working directory: with a rootful runtime that means the tree is owned by this
    /// uid (the worker does it when it runs as root); with a rootless one container root (`0:0`) *is* the
    /// host user. [`probe`](OciSandbox::probe) reports which kind of runtime this is.
    pub user: String,
    /// `--cpus`, the number of CPU cores' worth of time the container may use; `None` for no limit.
    pub cpus: Option<f64>,
    /// Accept a tag-only image reference (mutable: the digest then binds a name, not the content).
    pub allow_unpinned_image: bool,
    /// The longest a housekeeping call to the runtime (`kill`, `inspect`, `info`, `image inspect`) may take.
    pub control_timeout: Duration,
    /// The longest removing the container may take.
    pub remove_timeout: Duration,
}

impl OciConfig {
    /// Defaults: user `65534:65534`, one CPU, digest-pinned image required, 10 s for housekeeping calls and
    /// 20 s for removing the container.
    pub fn new(runtime: impl Into<String>, image: impl Into<String>) -> Self {
        OciConfig {
            runtime: runtime.into(),
            image: image.into(),
            user: "65534:65534".into(),
            cpus: Some(1.0),
            allow_unpinned_image: false,
            control_timeout: DEFAULT_CONTROL_TIMEOUT,
            remove_timeout: DEFAULT_REMOVE_TIMEOUT,
        }
    }
}

/// What a container runtime says about itself ([`OciSandbox::probe`]). `None`: it does not report it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeInfo {
    /// The server's version string.
    pub version: Option<String>,
    /// The runtime runs without root privileges (rootless Podman or Docker): container root is the host user.
    pub rootless: Option<bool>,
    /// Containers run in a user namespace (rootless, or `userns-remap`).
    pub user_ns: Option<bool>,
    /// A seccomp profile is applied to containers by default.
    pub seccomp: Option<bool>,
}

impl RuntimeInfo {
    /// The `--user` value under which the container can write a working tree that is owned by the user
    /// running this process (`euid`:`egid`), or `None` when no user can: a **rootful** runtime maps container
    /// ids to the same host ids, so a non-root caller needs the container to run as itself, while a **rootless**
    /// one maps container root (`0:0`) to the caller — and nothing else of the caller's. A root caller chowns
    /// the tree to whatever `root_default` names instead (see `ag-worker`), so this returns `root_default`.
    pub fn user_for_tree(&self, euid: u32, egid: u32, root_default: &str) -> String {
        if self.rootless == Some(true) {
            "0:0".to_string()
        } else if euid != 0 {
            format!("{euid}:{egid}")
        } else {
            root_default.to_string()
        }
    }
}

/// Runs commands in a container through `docker` or `podman`. See the module documentation for what is and
/// is not verified.
#[derive(Clone, Debug)]
pub struct OciSandbox {
    cfg: OciConfig,
}

impl OciSandbox {
    /// A sandbox with default settings (see [`OciConfig::new`]).
    pub fn new(runtime: impl Into<String>, image: impl Into<String>) -> Result<Self, SandboxError> {
        Self::with_config(OciConfig::new(runtime, image))
    }

    /// A sandbox with explicit settings. Fails if the image reference is malformed or not pinned.
    pub fn with_config(cfg: OciConfig) -> Result<Self, SandboxError> {
        if cfg.runtime.is_empty() || cfg.runtime.contains(char::is_whitespace) || cfg.runtime.starts_with('-') {
            return Err(SandboxError::invalid(format!("bad container runtime {:?}", cfg.runtime)));
        }
        if cfg.image.is_empty()
            || cfg.image.contains(char::is_whitespace)
            || cfg.image.starts_with('-')
            || cfg.image.contains('\0')
        {
            return Err(SandboxError::invalid(format!("bad image reference {:?}", cfg.image)));
        }
        if !cfg.allow_unpinned_image && !is_digest_pinned(&cfg.image) {
            return Err(SandboxError::invalid(format!(
                "image {:?} is not pinned by digest (name@sha256:<64 hex digits>); a tag can change under an attestation. \
                 Pin it, or set allow_unpinned_image to accept the weaker statement",
                cfg.image
            )));
        }
        if cfg.user.is_empty() || cfg.user.contains(char::is_whitespace) || cfg.user.starts_with('-') {
            return Err(SandboxError::invalid(format!("bad container user {:?}", cfg.user)));
        }
        if cfg.cpus.is_some_and(|c| !(c.is_finite() && c > 0.0 && c <= 4096.0)) {
            return Err(SandboxError::invalid("cpus must be a positive number"));
        }
        if cfg.control_timeout.is_zero() || cfg.remove_timeout.is_zero() {
            return Err(SandboxError::invalid("the runtime call timeouts must not be zero"));
        }
        Ok(OciSandbox { cfg })
    }

    /// Whether the configured runtime executable can be found. This is cheap and only that: the daemon's
    /// reachability and the image's presence are *not* probed here (use [`probe`](Self::probe) and
    /// [`image_present`](Self::image_present), which are bounded by `control_timeout`).
    pub fn availability(&self) -> crate::Availability {
        match find_executable(&self.cfg.runtime) {
            Some(_) => crate::Availability::Available,
            None => crate::Availability::Unavailable(format!(
                "container runtime `{}` was not found on PATH",
                self.cfg.runtime
            )),
        }
    }

    /// Ask the runtime about itself (`<runtime> info`): is the daemon reachable, is it rootless, does it
    /// apply a seccomp profile. Bounded by `control_timeout`. An unreachable daemon is
    /// [`SandboxError::Unavailable`] with the runtime's own message.
    pub fn probe(&self) -> Result<RuntimeInfo, SandboxError> {
        probe_runtime(&self.cfg.runtime, self.cfg.control_timeout)
    }

    /// Whether the configured image is in the runtime's local store. The sandbox never pulls, so a job whose
    /// image is missing cannot run: pull it first (`<runtime> pull <image>`), by digest.
    pub fn image_present(&self) -> Result<(), SandboxError> {
        let runtime = self.runtime_path()?;
        match capture(&runtime, &["image", "inspect", "--format", "{{.Id}}", &self.cfg.image], self.cfg.control_timeout)
        {
            Some(c) if c.ok => Ok(()),
            Some(c) => Err(SandboxError::Runtime(format!(
                "image {} is not in the local store of `{}` ({}); the sandbox never pulls: run `{} pull {}` first",
                self.cfg.image,
                self.cfg.runtime,
                failure_text(&self.cfg.runtime, &c),
                self.cfg.runtime,
                self.cfg.image
            ))),
            None => Err(SandboxError::Runtime(format!(
                "`{} image inspect` did not answer within {:?}",
                self.cfg.runtime, self.cfg.control_timeout
            ))),
        }
    }

    /// Remove containers a crashed or killed worker on **this host** left behind; see [`reap_orphans`].
    pub fn reap_orphans(&self) -> Result<Vec<String>, SandboxError> {
        reap_orphans(&self.cfg.runtime, self.cfg.control_timeout, self.cfg.remove_timeout)
    }

    fn runtime_path(&self) -> Result<PathBuf, SandboxError> {
        find_executable(&self.cfg.runtime).ok_or_else(|| SandboxError::Unavailable {
            sandbox: "oci",
            reason: format!("container runtime `{}` was not found on PATH", self.cfg.runtime),
        })
    }

    /// The `run` arguments for `spec` (after the runtime executable's own name), for inspection and tests.
    /// `env_file` is the path of the file carrying the command's environment, if it has one.
    pub fn run_args(
        &self,
        spec: &SandboxSpec,
        container_name: &str,
        env_file: Option<&Path>,
    ) -> Result<Vec<String>, SandboxError> {
        spec.validate()?;
        let l = &spec.limits;
        let mut a: Vec<String> = Vec::new();
        let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
        push(&["run", "--name", container_name, "--label", LABEL, "--pull", "never"]);
        let host = sys::hostname();
        if !host.is_empty() {
            push(&["--label", &format!("{HOST_LABEL}={host}")]);
        }
        push(&["--network", if spec.network == Network::None { "none" } else { "host" }]);
        push(&["--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges"]);
        push(&["--user", &self.cfg.user]);
        push(&["--pids-limit", &l.pids.to_string()]);
        push(&["--memory", &l.memory_bytes.to_string(), "--memory-swap", &l.memory_bytes.to_string()]);
        if let Some(cpus) = self.cfg.cpus {
            push(&["--cpus", &format!("{cpus}")]);
        }
        for (name, soft, hard) in ulimits(spec) {
            push(&["--ulimit", &format!("{name}={soft}:{hard}")]);
        }
        push(&["--tmpfs", &format!("/tmp:rw,nosuid,nodev,size={}", spec.rw_tmp_bytes)]);
        push(&["--mount", &bind_mount(&spec.workdir, Path::new(WORK_DIR), false)?]);
        for m in &spec.ro_mounts {
            push(&["--mount", &bind_mount(&m.host, &normalize_sandbox_path(&m.sandbox)?, true)?]);
        }
        push(&["--workdir", WORK_DIR]);
        if let Some(f) = env_file {
            push(&["--env-file", &f.to_string_lossy()]);
        }
        if !spec.stdin.is_empty() {
            push(&["-i"]);
        }
        // Run exactly argv, whatever ENTRYPOINT the image declares.
        push(&["--entrypoint", &spec.argv[0]]);
        push(&[&self.cfg.image]);
        a.extend(spec.argv[1..].iter().cloned());
        Ok(a)
    }

    /// What was asked of the runtime, in the terms `inspect` reports it.
    fn requested<'a>(&'a self, spec: &SandboxSpec) -> Result<Requested<'a>, SandboxError> {
        let mut binds = vec![(WORK_DIR.to_string(), true)];
        for m in &spec.ro_mounts {
            binds.push((normalize_sandbox_path(&m.sandbox)?.to_string_lossy().into_owned(), false));
        }
        Ok(Requested {
            user: &self.cfg.user,
            network_none: spec.network == Network::None,
            pids: spec.limits.pids,
            memory_bytes: spec.limits.memory_bytes,
            cpus: self.cfg.cpus,
            ulimits: ulimits(spec),
            binds,
        })
    }
}

/// Ask the container runtime `runtime` (`docker`, `podman` or a path) about itself, without an image: is the
/// daemon reachable, is it rootless, does it apply a seccomp profile by default. `timeout` bounds the call. An
/// unreachable daemon is [`SandboxError::Unavailable`] carrying the runtime's own message.
pub fn probe_runtime(runtime: &str, timeout: Duration) -> Result<RuntimeInfo, SandboxError> {
    let path = find_executable(runtime).ok_or_else(|| SandboxError::Unavailable {
        sandbox: "oci",
        reason: format!("container runtime `{runtime}` was not found on PATH"),
    })?;
    match capture(&path, &["info", "--format", "{{json .}}"], timeout) {
        Some(c) if c.ok => {
            let doc: serde_json::Value = serde_json::from_str(c.stdout.trim()).unwrap_or_default();
            let facts = oci_record::daemon_facts(&doc);
            let version = doc
                .get("ServerVersion")
                .or_else(|| doc.pointer("/version/Version"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Ok(RuntimeInfo { version, rootless: facts.rootless, user_ns: facts.user_ns, seccomp: facts.seccomp })
        }
        Some(c) => Err(SandboxError::Unavailable { sandbox: "oci", reason: failure_text(runtime, &c) }),
        None => Err(SandboxError::Unavailable {
            sandbox: "oci",
            reason: format!(
                "`{runtime} info` did not answer within {timeout:?} (is the daemon running and reachable?)"
            ),
        }),
    }
}

/// Remove containers a crashed or killed worker on **this host** left behind: those labelled `ag-sandbox=1`
/// with this host's name whose starting process (the pid in the container's name) no longer exists. Returns the
/// names removed. `control` bounds the listing, `remove` each removal.
///
/// Assumes that every process that starts containers on this host shares one process-id namespace (the host
/// name is part of the label so that workers in separate containers, which have their own host names, never
/// reap each other's containers).
pub fn reap_orphans(runtime: &str, control: Duration, remove_timeout: Duration) -> Result<Vec<String>, SandboxError> {
    let path = find_executable(runtime).ok_or_else(|| SandboxError::Unavailable {
        sandbox: "oci",
        reason: format!("container runtime `{runtime}` was not found on PATH"),
    })?;
    let host = sys::hostname();
    let mut args = vec!["ps".to_string(), "-a".into(), "--filter".into(), format!("label={LABEL}")];
    if !host.is_empty() {
        args.extend(["--filter".to_string(), format!("label={HOST_LABEL}={host}")]);
    }
    args.extend(["--format".to_string(), "{{.Names}}".into()]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = match capture(&path, &args, control) {
        Some(c) if c.ok => c.stdout,
        Some(c) => return Err(SandboxError::Runtime(failure_text(runtime, &c))),
        None => return Err(SandboxError::Runtime(format!("`{runtime} ps` did not answer in time"))),
    };
    let mut removed = Vec::new();
    for name in listing.split_whitespace() {
        let Some(pid) = owner_pid(name) else { continue };
        if !sys::process_exists(pid) && remove(&path, name, remove_timeout) {
            removed.push(name.to_string());
        }
    }
    Ok(removed)
}

/// The `--ulimit` settings of a spec: `(name, soft, hard)`.
///
/// The CPU limit's hard value is one second above the soft one: with equal values the kernel kills with
/// `SIGKILL` at the limit (indistinguishable from an OOM kill or a `docker kill`), while a lower soft limit
/// sends `SIGXCPU`, which is what [`ExitStatus::CpuLimit`] is recognised by. (The same pair the namespace
/// sandbox uses for `RLIMIT_CPU`.)
fn ulimits(spec: &SandboxSpec) -> Vec<(&'static str, u64, u64)> {
    let l = &spec.limits;
    vec![
        ("nofile", l.open_files, l.open_files),
        ("fsize", l.file_size_bytes, l.file_size_bytes),
        ("cpu", l.cpu_s, l.cpu_s.saturating_add(1)),
        ("core", 0, 0),
    ]
}

fn is_digest_pinned(image: &str) -> bool {
    match image.rsplit_once("@sha256:") {
        Some((name, hex)) => !name.is_empty() && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

/// `--mount type=bind,...`. The option syntax is comma separated, so paths with commas or quotes are refused
/// rather than escaped.
fn bind_mount(host: &Path, target: &Path, readonly: bool) -> Result<String, SandboxError> {
    let (h, t) = (host.to_string_lossy(), target.to_string_lossy());
    for p in [&h, &t] {
        if p.contains([',', '"', '\n']) {
            return Err(SandboxError::invalid(format!(
                "path {p:?} contains a character the container runtime's mount syntax cannot carry (comma, quote or newline)"
            )));
        }
    }
    Ok(format!("type=bind,src={h},dst={t}{}", if readonly { ",readonly" } else { "" }))
}

fn find_executable(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// A fresh container name: the starting process's id (so that [`OciSandbox::reap_orphans`] can tell whether
/// its owner is alive), a counter and a clock-derived suffix.
fn container_name() -> String {
    format!(
        "{NAME_PREFIX}{}-{}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)
    )
}

/// The pid in a name made by [`container_name`].
fn owner_pid(name: &str) -> Option<libc::pid_t> {
    name.strip_prefix(NAME_PREFIX)?.split('-').next()?.parse().ok()
}

impl Sandbox for OciSandbox {
    fn name(&self) -> &'static str {
        "oci"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::Container
    }

    fn preflight(&self) -> Result<(), SandboxError> {
        self.image_present()
    }

    fn run_cancellable(&self, spec: &SandboxSpec, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, SandboxError> {
        spec.validate()?;
        let runtime = self.runtime_path()?;
        let started = Instant::now();
        let name = container_name();
        let requested = self.requested(spec)?;

        let env_file = EnvFile::create(spec)?;
        let args = self.run_args(spec, &name, env_file.as_ref().map(|f| f.path.as_path()))?;

        let mut cmd = client_command(&runtime);
        cmd.args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SandboxError::Unavailable {
                    sandbox: "oci",
                    reason: format!("cannot execute {}: {e}", runtime.display()),
                });
            }
            Err(e) => return Err(SandboxError::setup(format!("start {}", runtime.display()), e)),
        };
        let pid = child.id() as libc::pid_t;
        let io = crate::spawn::Io {
            stdin: child.stdin.take().map(OwnedFd::from),
            stdout: OwnedFd::from(child.stdout.take().expect("piped")),
            stderr: OwnedFd::from(child.stderr.take().expect("piped")),
            status: None,
        };
        for fd in [Some(&io.stdout), Some(&io.stderr), io.stdin.as_ref()].into_iter().flatten() {
            sys::set_nonblocking(fd).map_err(|e| SandboxError::io("preparing the runtime client's pipes", e))?;
        }
        let pidfd = sys::pidfd_open(pid);

        // Killing the client does not stop the container; ask the runtime to.
        let kill_runtime = runtime.clone();
        let kill_name = name.clone();
        let control = self.cfg.control_timeout;
        let mut on_kill = move || {
            let _ = capture(&kill_runtime, &["kill", &kill_name], control);
        };
        let result = supervise(
            pid,
            pidfd.as_ref(),
            io,
            &spec.stdin,
            &spec.limits,
            started,
            Hooks { group: true, on_kill: Some(&mut on_kill), sampler: None, task_limit: None, cancel: Some(cancel) },
        );
        // The supervisor reaped the client itself; dropping std's handle must not wait again.
        std::mem::forget(child);
        let finished = match result {
            Ok(f) => f,
            Err(e) => {
                remove(&runtime, &name, self.cfg.remove_timeout);
                return Err(e);
            }
        };

        // What the runtime recorded about the container and how it ended: asked before the container is
        // removed (it is the only place the exit state, the OOM kill and the settings can be read from).
        let inspect = capture(&runtime, &["inspect", "--type", "container", &name], control)
            .filter(|c| c.ok)
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c.stdout.trim()).ok());
        let removed = remove(&runtime, &name, self.cfg.remove_timeout);
        drop(env_file);
        let daemon = capture(&runtime, &["info", "--format", "{{json .}}"], control)
            .filter(|c| c.ok)
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c.stdout.trim()).ok())
            .map(|d| oci_record::daemon_facts(&d))
            .unwrap_or_default();

        let record = inspect.as_ref().and_then(oci_record::record);
        let observed = record.map(|r| oci_record::observe(r, &requested));
        if let Some(o) = &observed {
            let problems = o.violations();
            if !problems.is_empty() {
                return Err(SandboxError::Runtime(format!(
                    "`{}` did not create the container as requested, so no verdict can be given for it: {}",
                    self.cfg.runtime,
                    problems.join("; ")
                )));
            }
        }
        let state = record.and_then(oci_record::state);
        let status = map_status(&finished, state.as_ref(), &spec.argv[0], &self.cfg.runtime)?;

        let isolation = self.report(spec, observed.as_ref(), record.is_some(), daemon, removed, &name);
        Ok(Outcome {
            status,
            stdout: finished.stdout,
            stderr: finished.stderr,
            stdout_truncated: finished.stdout_truncated,
            stderr_truncated: finished.stderr_truncated,
            wall_ms: finished.wall.as_millis() as u64,
            cpu_ms: None,
            max_rss_bytes: None,
            environment_digest: environment_digest("oci", &isolation, spec),
            isolation,
        })
    }
}

impl OciSandbox {
    /// The report of a run: every protection is claimed only if the runtime's record of the container (and,
    /// for seccomp and user namespaces, its description of itself) says so.
    fn report(
        &self,
        spec: &SandboxSpec,
        observed: Option<&Observed>,
        record_read: bool,
        daemon: DaemonFacts,
        removed: bool,
        name: &str,
    ) -> IsolationReport {
        let holds = |f: fn(&Observed) -> &Check| observed.is_some_and(|o| f(o).holds());
        let memory_set = holds(|o| &o.memory);
        let pids_set = holds(|o| &o.pids);
        let user_ns = daemon.user_ns == Some(true);
        // Container root is host root unless the runtime maps the container into a user namespace; a user the
        // image resolves by name is treated as root because it is not known.
        let host_root = !user_ns && oci_record::numeric_uid(&self.cfg.user).is_none_or(|uid| uid == 0);
        let mounts = holds(|o| &o.mounts) && holds(|o| &o.unprivileged);
        let namespaces = holds(|o| &o.private_namespaces);
        let verified = observed.is_some_and(Observed::all_protections_hold)
            && daemon.seccomp.is_some()
            && daemon.user_ns.is_some();
        let mut notes = vec![
            if verified {
                "read back from the container runtime's own record of the container (`inspect`) and its `info`; the kernel's enforcement is not observed per run (it is exercised by the real-runtime tests in CI)"
                    .to_string()
            } else {
                "NOT fully read back from the container runtime: the flags are requested, and only the properties listed as held below are claimed"
                    .to_string()
            },
            if is_digest_pinned(&self.cfg.image) {
                "image is pinned by digest".into()
            } else {
                "image is NOT pinned by digest".into()
            },
        ];
        match observed {
            Some(o) => notes.extend(o.caveats().into_iter().map(|c| format!("runtime record: {c}"))),
            None if !record_read => notes.push("the runtime's record of the container could not be read".into()),
            None => {}
        }
        if daemon.seccomp.is_none() || daemon.user_ns.is_none() {
            notes.push("the runtime did not report its seccomp or user-namespace settings".into());
        }
        if daemon.seccomp == Some(false) {
            notes.push("the runtime applies no seccomp profile by default".into());
        }
        if !removed {
            notes.push(format!("the container {name} could not be removed (label {LABEL}); `reap_orphans` removes it"));
        }
        IsolationReport {
            verified,
            user_ns,
            mount_ns: mounts,
            pid_ns: namespaces,
            net_ns: spec.network == Network::None && holds(|o| &o.network),
            ipc_ns: namespaces,
            uts_ns: namespaces,
            private_root: mounts,
            read_only_root: holds(|o| &o.read_only_root),
            no_new_privs: holds(|o| &o.no_new_privs),
            capabilities_dropped: holds(|o| &o.capabilities_dropped),
            seccomp: daemon.seccomp == Some(true),
            rlimits: holds(|o| &o.rlimits),
            host_root,
            memory: if memory_set { Enforcement::Runtime } else { Enforcement::None },
            tasks: if pids_set { Enforcement::Runtime } else { Enforcement::None },
            cgroup: (memory_set || pids_set).then_some(CgroupReport { memory: memory_set, pids: pids_set }),
            network: spec.network,
            image: Some(self.cfg.image.clone()),
            runtime: Some(self.cfg.runtime.clone()),
            notes,
        }
    }
}

/// The runtime client's own environment: a small allow-list of what it needs to find its daemon.
fn client_command(runtime: &Path) -> Command {
    let mut cmd = Command::new(runtime);
    cmd.env_clear();
    for key in CLIENT_ENV {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd
}

/// What a short runtime command printed.
struct Captured {
    /// It exited with status 0.
    ok: bool,
    stdout: String,
    stderr: String,
}

/// The first line of a failed runtime command's stderr (or its stdout), for messages.
fn failure_text(runtime: &str, c: &Captured) -> String {
    let text = if c.stderr.trim().is_empty() { c.stdout.trim() } else { c.stderr.trim() };
    let line = text.lines().next().unwrap_or("no output");
    format!("`{runtime}` said: {line}")
}

/// Run a short runtime command and collect its output; `None` if it could not be run or took too long.
fn capture(runtime: &Path, args: &[&str], limit: Duration) -> Option<Captured> {
    let mut cmd = client_command(runtime);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn().ok()?;
    let out = read_capped(child.stdout.take());
    let err = read_capped(child.stderr.take());
    let deadline = Instant::now() + limit;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // A timed-out command's partial output is not an answer.
                let _ = (out.join(), err.join());
                return None;
            }
        }
    };
    Some(Captured { ok, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

/// Read a pipe to the end in a thread, keeping at most [`CAPTURE_LIMIT`] bytes.
fn read_capped<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = pipe {
            let _ = std::io::Read::read_to_string(&mut std::io::Read::take(p, CAPTURE_LIMIT), &mut s);
        }
        s
    })
}

/// Force-remove the container (also the way a killed or timed-out one is finally cleaned up). `false` if the
/// runtime did not confirm it within the timeout.
fn remove(runtime: &Path, name: &str, limit: Duration) -> bool {
    capture(runtime, &["rm", "-f", "-v", name], limit).is_some_and(|c| c.ok)
}

/// Docker and Podman return the container command's exit status, but use 125 for their own failures
/// (daemon unreachable, bad flag, image missing) and 126/127 when the command could not be started. The
/// runtime's record of the container tells the two apart: a container that was only *created* never ran its
/// command, whatever the code says; one that exited ran it, and the code is the command's own (a check that
/// exits 125 after printing `Error: ` is a failed check, not a runtime failure). Only when the record
/// cannot be read does the runtime's stderr decide.
fn map_status(f: &Finished, state: Option<&State>, argv0: &str, runtime: &str) -> Result<ExitStatus, SandboxError> {
    match f.verdict {
        Some(Verdict::TimedOut) => return Ok(ExitStatus::TimedOut),
        Some(Verdict::OutputLimit) => return Ok(ExitStatus::OutputLimit),
        Some(Verdict::MemoryLimit) => return Ok(ExitStatus::MemoryLimit),
        Some(Verdict::PidsLimit) => return Ok(ExitStatus::PidsLimit),
        None => {}
    }
    let natural = wait_status(f.reaped.status);
    let stderr = String::from_utf8_lossy(&f.stderr);
    match state {
        Some(st) if st.status == "created" => {
            // The runtime made the container but could not start its process.
            let why = if st.error.is_empty() { stderr.trim().to_string() } else { st.error.trim().to_string() };
            return Err(match natural {
                ExitStatus::Exited(126 | 127) => SandboxError::Exec {
                    program: argv0.to_string(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, why),
                },
                _ => SandboxError::Runtime(why),
            });
        }
        Some(st) => {
            if st.oom_killed && !natural.success() {
                return Ok(ExitStatus::MemoryLimit);
            }
        }
        None => {
            if let ExitStatus::Exited(125) = natural {
                const RUNTIME_ERRORS: [&str; 7] = [
                    "Error response from daemon",
                    "Cannot connect to the Docker daemon",
                    "Unable to find image",
                    "pull access denied",
                    "invalid reference format",
                    "unknown flag",
                    "unknown shorthand flag",
                ];
                let from_runtime = stderr.starts_with(&format!("{runtime}:"))
                    || stderr.starts_with("Error: ")
                    || RUNTIME_ERRORS.iter().any(|m| stderr.contains(m));
                if from_runtime {
                    return Err(SandboxError::Runtime(stderr.trim().to_string()));
                }
            }
            // 137 is both "killed by the OOM killer" and an ordinary kill; without the record it cannot be
            // told which, and signing a guess would put a wrong cause into an attestation.
            if matches!(natural, ExitStatus::Exited(137) | ExitStatus::Signaled(libc::SIGKILL)) {
                return Err(SandboxError::Runtime(
                    "the container was killed (exit status 137) and its state could not be read back from the runtime, so an out-of-memory kill cannot be told from another kill"
                        .into(),
                ));
            }
        }
    }
    // `--ulimit cpu` ends the container's process with SIGXCPU, which the client reports as 128 + 24.
    if natural == ExitStatus::Exited(128 + libc::SIGXCPU) {
        return Ok(ExitStatus::CpuLimit);
    }
    Ok(natural)
}

/// A private file holding the command's environment, deleted when dropped.
struct EnvFile {
    path: PathBuf,
}

impl EnvFile {
    fn create(spec: &SandboxSpec) -> Result<Option<EnvFile>, SandboxError> {
        if spec.env.is_empty() {
            return Ok(None);
        }
        let mut body = String::new();
        for (k, v) in &spec.env {
            // The file format is one `NAME=value` per line, taken literally; there is no way to carry a
            // newline, and a leading `#` would make the line a comment.
            if k.starts_with('#') || k.contains(char::is_whitespace) || v.contains(['\n', '\r']) {
                return Err(SandboxError::invalid(format!(
                    "environment variable {k:?} cannot be passed to a container runtime (newline, whitespace in the name, or a leading #)"
                )));
            }
            body.push_str(&format!("{k}={v}\n"));
        }
        let path = std::env::temp_dir().join(format!(
            "ag-sandbox-env-{}-{}-{:x}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)
        ));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| SandboxError::setup("create the environment file", e))?;
        let file = EnvFile { path };
        f.write_all(body.as_bytes()).map_err(|e| SandboxError::setup("write the environment file", e))?;
        Ok(Some(file))
    }
}

impl Drop for EnvFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Mount;

    const PINNED: &str =
        "registry.example/build@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn args_for(spec: &SandboxSpec) -> Vec<String> {
        OciSandbox::new("docker", PINNED).unwrap().run_args(spec, "ag-test", None).unwrap()
    }

    fn pair(a: &[String], flag: &str) -> Vec<String> {
        a.windows(2).filter(|w| w[0] == flag).map(|w| w[1].clone()).collect()
    }

    #[test]
    fn the_command_line_carries_every_restriction() {
        let dir = std::env::temp_dir();
        let mut spec = SandboxSpec::new(["cargo", "test", "--", "-q"], &dir).ro_mount(Mount::new("/usr", "/opt/usr"));
        spec.limits.pids = 33;
        spec.limits.memory_bytes = 512 << 20;
        spec.rw_tmp_bytes = 8 << 20;
        let a = args_for(&spec);
        assert_eq!(a[0], "run");
        assert_eq!(pair(&a, "--network"), ["none"]);
        assert!(a.contains(&"--read-only".to_string()));
        assert_eq!(pair(&a, "--cap-drop"), ["ALL"]);
        assert_eq!(pair(&a, "--security-opt"), ["no-new-privileges"]);
        assert_eq!(pair(&a, "--pids-limit"), ["33"]);
        assert_eq!(pair(&a, "--memory"), [(512u64 << 20).to_string()]);
        assert_eq!(pair(&a, "--memory-swap"), [(512u64 << 20).to_string()], "no swap");
        assert_eq!(pair(&a, "--user"), ["65534:65534"]);
        assert_eq!(pair(&a, "--pull"), ["never"]);
        assert_eq!(pair(&a, "--workdir"), ["/work"]);
        assert_eq!(pair(&a, "--tmpfs"), [format!("/tmp:rw,nosuid,nodev,size={}", 8 << 20)]);
        let mounts = pair(&a, "--mount");
        assert_eq!(mounts[0], format!("type=bind,src={},dst=/work", dir.display()));
        assert_eq!(mounts[1], "type=bind,src=/usr,dst=/opt/usr,readonly");
        let ulimits = pair(&a, "--ulimit");
        for want in ["nofile=1024:1024", "fsize=1073741824:1073741824", "cpu=60:61", "core=0:0"] {
            assert!(ulimits.contains(&want.to_string()), "{want} in {ulimits:?}");
        }
        // the image comes right before the arguments, and the entrypoint is replaced by argv[0]
        let i = a.iter().position(|x| x == PINNED).unwrap();
        assert_eq!(a[i + 1..], ["test", "--", "-q"]);
        assert_eq!(pair(&a, "--entrypoint"), ["cargo"]);
        assert!(!a.contains(&"-i".to_string()), "no stdin, no -i");
        for forbidden in
            ["--privileged", "--cap-add", "--device", "--pid", "--ipc", "--userns", "-v", "--volume", "-e", "--env"]
        {
            assert!(!a.contains(&forbidden.to_string()), "{forbidden}");
        }
    }

    #[test]
    fn host_network_and_stdin_are_explicit() {
        let dir = std::env::temp_dir();
        let a = args_for(&SandboxSpec::new(["sh"], &dir).network(Network::Host).stdin(b"x".to_vec()));
        assert_eq!(pair(&a, "--network"), ["host"]);
        assert!(a.contains(&"-i".to_string()));
    }

    #[test]
    fn the_environment_goes_through_a_private_file_not_the_command_line() {
        let dir = std::env::temp_dir();
        let spec = SandboxSpec::new(["sh"], &dir).env("TOKEN_LIKE", "s3cr3t-value").env("A", "b c");
        let file = EnvFile::create(&spec).unwrap().unwrap();
        let a = OciSandbox::new("docker", PINNED).unwrap().run_args(&spec, "n", Some(&file.path)).unwrap();
        assert!(!a.iter().any(|x| x.contains("s3cr3t-value")));
        assert_eq!(pair(&a, "--env-file"), [file.path.to_string_lossy().to_string()]);
        let body = std::fs::read_to_string(&file.path).unwrap();
        assert_eq!(body, "A=b c\nTOKEN_LIKE=s3cr3t-value\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777, 0o600);
        let path = file.path.clone();
        drop(file);
        assert!(!path.exists());
        // values that cannot be expressed are refused instead of being mangled
        assert!(EnvFile::create(&SandboxSpec::new(["sh"], &dir).env("A", "x\ny")).is_err());
    }

    #[test]
    fn images_must_be_pinned_unless_the_caller_says_otherwise() {
        assert!(OciSandbox::new("docker", PINNED).is_ok());
        assert!(OciSandbox::new("docker", "ubuntu:24.04").is_err());
        assert!(OciSandbox::new("docker", "ubuntu@sha256:abc").is_err());
        assert!(OciSandbox::new("docker", "--privileged").is_err());
        assert!(OciSandbox::new("dock er", PINNED).is_err());
        let mut cfg = OciConfig::new("podman", "ubuntu:24.04");
        cfg.allow_unpinned_image = true;
        assert!(OciSandbox::with_config(cfg).is_ok());
    }

    #[test]
    fn paths_the_mount_syntax_cannot_carry_are_refused() {
        assert!(bind_mount(Path::new("/a,b"), Path::new("/x"), true).is_err());
        assert!(bind_mount(Path::new("/a"), Path::new("/x\"y"), true).is_err());
    }

    fn finished(code: i32, stderr: &str) -> Finished {
        Finished {
            // a wait status for "exited with `code`"
            reaped: sys::Reaped { status: code << 8, rusage: sys::zero_rusage() },
            verdict: None,
            failure: None,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_truncated: false,
            stderr_truncated: false,
            peak_sampled: None,
            wall: Duration::ZERO,
        }
    }

    fn state(status: &str, exit: i64, oom: bool, error: &str) -> State {
        State { status: status.into(), exit_code: Some(exit), oom_killed: oom, error: error.into() }
    }

    fn status_of(f: &Finished, st: Option<&State>) -> Result<ExitStatus, SandboxError> {
        map_status(f, st, "sh", "docker")
    }

    #[test]
    fn what_the_runtime_recorded_decides_whether_the_command_ran() {
        // exited: the code is the command's own, even 125 with "Error: " on stderr (real Docker, finding 40)
        let ran = state("exited", 125, false, "");
        assert_eq!(status_of(&finished(125, "Error: boom"), Some(&ran)).unwrap(), ExitStatus::Exited(125));
        let ran = state("exited", 126, false, "");
        assert_eq!(status_of(&finished(126, ""), Some(&ran)).unwrap(), ExitStatus::Exited(126));
        // created: the runtime never started the command
        let never = state("created", 127, false, "exec: \"nosuch\": executable file not found in $PATH");
        match map_status(&finished(127, "docker: Error response from daemon: boom"), Some(&never), "nosuch", "docker") {
            Err(SandboxError::Exec { program, source }) => {
                assert_eq!(program, "nosuch");
                assert!(source.to_string().contains("executable file not found"), "{source}");
            }
            other => panic!("{other:?}"),
        }
        let refused = state("created", 125, false, "OCI runtime create failed");
        assert!(
            matches!(status_of(&finished(125, ""), Some(&refused)), Err(SandboxError::Runtime(m)) if m.contains("create failed"))
        );
        // OOM kill: only the runtime's record can say it
        let oom = state("exited", 137, true, "");
        assert_eq!(status_of(&finished(137, ""), Some(&oom)).unwrap(), ExitStatus::MemoryLimit);
        let plain = state("exited", 137, false, "");
        assert_eq!(status_of(&finished(137, ""), Some(&plain)).unwrap(), ExitStatus::Exited(137));
        let fine = state("exited", 0, true, "");
        assert_eq!(status_of(&finished(0, ""), Some(&fine)).unwrap(), ExitStatus::Exited(0));
        let cpu = state("exited", 152, false, "");
        assert_eq!(status_of(&finished(152, ""), Some(&cpu)).unwrap(), ExitStatus::CpuLimit);
    }

    #[test]
    fn without_the_record_only_the_runtimes_own_voice_is_believed_and_guesses_are_refused() {
        let runtime_said = |code, stderr: &str, runtime: &str| map_status(&finished(code, stderr), None, "sh", runtime);
        assert!(matches!(
            runtime_said(125, "docker: Error response from daemon: boom", "docker"),
            Err(SandboxError::Runtime(_))
        ));
        assert!(matches!(runtime_said(125, "Error: image not known", "podman"), Err(SandboxError::Runtime(_))));
        assert!(matches!(
            runtime_said(125, "Unable to find image 'x' locally", "docker"),
            Err(SandboxError::Runtime(_))
        ));
        // a script that merely prints "Error: " somewhere is not mistaken for the runtime
        assert_eq!(
            runtime_said(125, "my script said Error: and exited 125", "docker").unwrap(),
            ExitStatus::Exited(125)
        );
        assert_eq!(runtime_said(126, "x", "docker").unwrap(), ExitStatus::Exited(126));
        assert_eq!(runtime_said(152, "", "docker").unwrap(), ExitStatus::CpuLimit);
        // 137 could be an OOM kill; with no record to say, no verdict is signed on a guess
        assert!(matches!(runtime_said(137, "", "docker"), Err(SandboxError::Runtime(_))));
    }

    #[test]
    fn the_supervisors_verdicts_win_over_the_exit_code() {
        let mut f = finished(137, "");
        f.verdict = Some(Verdict::TimedOut);
        assert_eq!(status_of(&f, None).unwrap(), ExitStatus::TimedOut);
        f.verdict = Some(Verdict::OutputLimit);
        assert_eq!(status_of(&f, None).unwrap(), ExitStatus::OutputLimit);
    }

    #[test]
    fn orphans_are_recognised_by_the_pid_in_their_name() {
        assert_eq!(owner_pid("ag-sandbox-4242-7-1a2b"), Some(4242));
        assert_eq!(owner_pid(&container_name()), Some(std::process::id() as libc::pid_t));
        assert_eq!(owner_pid("someone-elses-container"), None);
        assert_eq!(owner_pid("ag-sandbox-notapid-1-2"), None);
        assert!(sys::process_exists(std::process::id() as libc::pid_t));
        assert!(!sys::process_exists(i32::MAX - 1));
    }

    #[test]
    fn the_report_claims_only_what_the_runtime_recorded() {
        let sb = OciSandbox::new("docker", PINNED).unwrap();
        let spec = SandboxSpec::new(["sh"], std::env::temp_dir());
        let nothing = DaemonFacts::default();
        // nothing could be read: nothing is claimed
        let r = sb.report(&spec, None, false, nothing, true, "n");
        assert!(!r.verified && !r.net_ns && !r.read_only_root && !r.seccomp && !r.no_new_privs);
        assert_eq!((r.memory, r.tasks, r.cgroup), (Enforcement::None, Enforcement::None, None));
        assert!(r.notes.iter().any(|n| n.contains("could not be read")), "{:?}", r.notes);
        // a root user in a rootful runtime is host root; in a user namespace it is not
        let mut cfg = OciConfig::new("docker", PINNED);
        cfg.user = "0:0".into();
        let root = OciSandbox::with_config(cfg).unwrap();
        assert!(root.report(&spec, None, false, nothing, true, "n").host_root);
        let rootless = DaemonFacts { user_ns: Some(true), rootless: Some(true), seccomp: Some(true) };
        assert!(!root.report(&spec, None, false, rootless, true, "n").host_root);
        assert!(root.report(&spec, None, false, rootless, true, "n").user_ns);
        // a user known only by name is treated as root
        let mut cfg = OciConfig::new("docker", PINNED);
        cfg.user = "nobody".into();
        let named = OciSandbox::with_config(cfg).unwrap();
        assert!(named.report(&spec, None, false, nothing, true, "n").host_root);
        // a container that could not be removed says so
        let leaked = sb.report(&spec, None, false, nothing, false, "n");
        assert!(leaked.notes.iter().any(|n| n.contains("could not be removed")));
    }
}
