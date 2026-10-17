//! Container isolation through a `docker` or `podman` executable.
//!
//! This sandbox builds the `run` command line and supervises the runtime client; the isolation itself is the
//! runtime's. It is the stronger option for hostile workloads (separate runtime-managed namespaces, a
//! default seccomp *allow-list*, AppArmor/SELinux where configured, cgroup limits enforced by the kernel),
//! at the price of a daemon (Docker) or a rootless-container setup (Podman) and a prepared image.
//!
//! **Untested against a real runtime.** No container runtime exists on the machine this was written on. The
//! argument construction and the mapping of runtime failures are covered by tests that run a fake runtime
//! script; whether a real `docker run` behaves as intended with these arguments has not been verified here.

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
use crate::outcome::{CgroupReport, Enforcement, ExitStatus, IsolationLevel, IsolationReport, Outcome};
use crate::spec::{Network, SandboxSpec, WORK_DIR, normalize_sandbox_path};
use crate::supervise::{Finished, Hooks, Verdict, supervise, wait_status};
use crate::{Sandbox, sys};

static COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// How to reach the container runtime and which image to run in.
#[derive(Clone, Debug, PartialEq)]
pub struct OciConfig {
    /// `docker`, `podman`, or a path to a compatible executable.
    pub runtime: String,
    /// The image reference. Must be pinned by digest (`name@sha256:<64 hex>`) unless
    /// [`allow_unpinned_image`](Self::allow_unpinned_image) is set, so that "the environment a check ran in"
    /// names one exact image.
    pub image: String,
    /// `--user` value; the default is the conventional unprivileged `65534:65534`. The user must be able to
    /// write the bind-mounted working directory (the host side's concern: ownership or `--userns` settings
    /// of the runtime).
    pub user: String,
    /// `--cpus`, the number of CPU cores' worth of time the container may use; `None` for no limit.
    pub cpus: Option<f64>,
    /// Accept a tag-only image reference (mutable: the digest then binds a name, not the content).
    pub allow_unpinned_image: bool,
}

impl OciConfig {
    /// Defaults: user `65534:65534`, one CPU, digest-pinned image required.
    pub fn new(runtime: impl Into<String>, image: impl Into<String>) -> Self {
        OciConfig {
            runtime: runtime.into(),
            image: image.into(),
            user: "65534:65534".into(),
            cpus: Some(1.0),
            allow_unpinned_image: false,
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
        Ok(OciSandbox { cfg })
    }

    /// Whether the configured runtime executable can be found. The daemon's reachability and the image's
    /// presence are *not* probed (either can take unbounded time when broken).
    pub fn availability(&self) -> crate::Availability {
        match find_executable(&self.cfg.runtime) {
            Some(_) => crate::Availability::Available,
            None => crate::Availability::Unavailable(format!(
                "container runtime `{}` was not found on PATH",
                self.cfg.runtime
            )),
        }
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
        push(&["run", "--name", container_name, "--label", "ag-sandbox=1", "--pull", "never"]);
        push(&["--network", if spec.network == Network::None { "none" } else { "host" }]);
        push(&["--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges"]);
        push(&["--user", &self.cfg.user]);
        push(&["--pids-limit", &l.pids.to_string()]);
        push(&["--memory", &l.memory_bytes.to_string(), "--memory-swap", &l.memory_bytes.to_string()]);
        if let Some(cpus) = self.cfg.cpus {
            push(&["--cpus", &format!("{cpus}")]);
        }
        push(&["--ulimit", &format!("nofile={0}:{0}", l.open_files)]);
        push(&["--ulimit", &format!("fsize={0}:{0}", l.file_size_bytes)]);
        push(&["--ulimit", &format!("cpu={0}:{0}", l.cpu_s)]);
        push(&["--ulimit", "core=0:0"]);
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

impl Sandbox for OciSandbox {
    fn name(&self) -> &'static str {
        "oci"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::Container
    }

    fn run(&self, spec: &SandboxSpec) -> Result<Outcome, SandboxError> {
        spec.validate()?;
        let Some(runtime) = find_executable(&self.cfg.runtime) else {
            return Err(SandboxError::Unavailable {
                sandbox: "oci",
                reason: format!("container runtime `{}` was not found on PATH", self.cfg.runtime),
            });
        };
        let started = Instant::now();
        let name = format!(
            "ag-sandbox-{}-{}-{:x}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)
        );

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
        let mut on_kill = move || {
            let _ = capture(&kill_runtime, &["kill", &kill_name], Duration::from_secs(10));
        };
        let result = supervise(
            pid,
            pidfd.as_ref(),
            io,
            &spec.stdin,
            &spec.limits,
            started,
            Hooks { group: true, on_kill: Some(&mut on_kill), sampler: None, task_limit: None },
        );
        // The supervisor reaped the client itself; dropping std's handle must not wait again.
        std::mem::forget(child);
        let finished = match result {
            Ok(f) => f,
            Err(e) => {
                cleanup(&runtime, &name);
                return Err(e);
            }
        };

        // Did the kernel's OOM killer end the container? Asked before the container is removed.
        let oom = capture(&runtime, &["inspect", "--format", "{{.State.OOMKilled}}", &name], Duration::from_secs(10))
            .is_some_and(|(ok, out)| ok && out.trim() == "true");
        cleanup(&runtime, &name);
        drop(env_file);

        let status = map_status(&finished, oom, &self.cfg.runtime)?;
        let isolation = IsolationReport {
            verified: false,
            user_ns: false,
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
            host_root: false,
            memory: Enforcement::Runtime,
            tasks: Enforcement::Runtime,
            cgroup: Some(CgroupReport { memory: true, pids: true }),
            network: spec.network,
            image: Some(self.cfg.image.clone()),
            runtime: Some(self.cfg.runtime.clone()),
            notes: vec![
                "requested from the container runtime, not observed by this crate: the flags are --network none, --read-only, --cap-drop ALL, \
                 --security-opt no-new-privileges, --pids-limit, --memory, --ulimit; the seccomp profile is the runtime's default"
                    .into(),
                if is_digest_pinned(&self.cfg.image) { "image is pinned by digest".into() } else { "image is NOT pinned by digest".into() },
            ],
        };
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

/// Run a short runtime command and collect its stdout; `None` if it could not be run or took too long.
fn capture(runtime: &Path, args: &[&str], limit: Duration) -> Option<(bool, String)> {
    let mut cmd = client_command(runtime);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0);
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        s
    });
    let deadline = Instant::now() + limit;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    Some((ok, reader.join().unwrap_or_default()))
}

/// Force-remove the container (also the way a killed or timed-out one is finally cleaned up).
fn cleanup(runtime: &Path, name: &str) {
    let _ = capture(runtime, &["rm", "-f", "-v", name], Duration::from_secs(20));
}

/// Docker and Podman return the container command's exit status, but use 125 for their own failures
/// (daemon unreachable, bad flag, image missing). A command that really exits 125 is indistinguishable by
/// code alone, so the runtime's message on stderr decides.
fn map_status(f: &Finished, oom: bool, runtime: &str) -> Result<ExitStatus, SandboxError> {
    match f.verdict {
        Some(Verdict::TimedOut) => return Ok(ExitStatus::TimedOut),
        Some(Verdict::OutputLimit) => return Ok(ExitStatus::OutputLimit),
        Some(Verdict::MemoryLimit) => return Ok(ExitStatus::MemoryLimit),
        Some(Verdict::PidsLimit) => return Ok(ExitStatus::PidsLimit),
        None => {}
    }
    let natural = wait_status(f.reaped.status);
    if let ExitStatus::Exited(125) = natural {
        let stderr = String::from_utf8_lossy(&f.stderr);
        const RUNTIME_ERRORS: [&str; 8] = [
            "Error response from daemon",
            "Cannot connect to the Docker daemon",
            "Unable to find image",
            "pull access denied",
            "invalid reference format",
            "unknown flag",
            "unknown shorthand flag",
            "Error: ",
        ];
        let from_runtime =
            stderr.starts_with(&format!("{runtime}:")) || RUNTIME_ERRORS.iter().any(|m| stderr.contains(m));
        if from_runtime {
            return Err(SandboxError::Runtime(stderr.trim().to_string()));
        }
    }
    if oom && !natural.success() {
        return Ok(ExitStatus::MemoryLimit);
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
        for want in ["nofile=1024:1024", "fsize=1073741824:1073741824", "cpu=60:60", "core=0:0"] {
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

    #[test]
    fn exit_code_125_is_a_runtime_failure_only_if_the_runtime_says_so() {
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
        assert!(matches!(
            map_status(&finished(125, "docker: Error response from daemon: boom"), false, "docker"),
            Err(SandboxError::Runtime(_))
        ));
        assert!(matches!(
            map_status(&finished(125, "Unable to find image 'x' locally"), false, "docker"),
            Err(SandboxError::Runtime(_))
        ));
        assert_eq!(
            map_status(&finished(125, "my script exited 125"), false, "docker").unwrap(),
            ExitStatus::Exited(125)
        );
        assert_eq!(map_status(&finished(126, "x"), false, "docker").unwrap(), ExitStatus::Exited(126));
        assert_eq!(map_status(&finished(137, ""), true, "docker").unwrap(), ExitStatus::MemoryLimit);
        assert_eq!(map_status(&finished(137, ""), false, "docker").unwrap(), ExitStatus::Exited(137));
        assert_eq!(map_status(&finished(152, ""), false, "docker").unwrap(), ExitStatus::CpuLimit);
        assert_eq!(map_status(&finished(0, ""), true, "docker").unwrap(), ExitStatus::Exited(0));
    }
}
