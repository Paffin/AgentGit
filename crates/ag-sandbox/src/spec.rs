use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::SandboxError;

/// Where the writable working tree appears inside every sandbox.
pub const WORK_DIR: &str = "/work";

/// Sandbox paths that belong to the sandbox itself and cannot be used as mount targets
/// (nor anything beneath them).
const RESERVED: [&str; 6] = ["/", "/proc", "/dev", "/tmp", WORK_DIR, "/oldroot"];

/// What a sandboxed command can reach on the network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    /// No network. The command sees an empty network namespace with only a loopback interface (up),
    /// or the container runtime's `--network none`. This is the default.
    #[default]
    None,
    /// Share the host's network namespace. **An explicit opt-in that weakens isolation**: the command can
    /// reach every service the host can, including loopback listeners, cloud metadata endpoints and
    /// abstract Unix sockets. Use only for code and destinations you already trust.
    Host,
}

/// A host path made visible inside the sandbox **read-only** (toolchains, system directories).
///
/// Both paths must be absolute. The host path must exist when the sandbox starts; it may be a directory or
/// a single file. The sandbox path must not be, or lie under, the sandbox's own locations
/// (`/`, `/proc`, `/dev`, `/tmp`, `/work`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mount {
    /// Absolute path on the host.
    pub host: PathBuf,
    /// Absolute path inside the sandbox.
    pub sandbox: PathBuf,
    /// A stable name for *what* is mounted — a toolchain version, an image digest, a content hash.
    ///
    /// It is the mount's identity in [`crate::environment_digest`]. Without it the digest can only bind
    /// the host *path*, which says nothing about the content (a weaker statement; supply one when
    /// attestations must be reproducible across machines).
    #[serde(default)]
    pub identity: Option<String>,
}

impl Mount {
    /// A read-only mount of `host` at `sandbox`.
    pub fn new(host: impl Into<PathBuf>, sandbox: impl Into<PathBuf>) -> Self {
        Mount { host: host.into(), sandbox: sandbox.into(), identity: None }
    }

    /// A read-only mount of `path` at the same path inside the sandbox.
    pub fn same_path(path: impl Into<PathBuf>) -> Self {
        let p = path.into();
        Mount { host: p.clone(), sandbox: p, identity: None }
    }

    /// Attach a content identity (see [`Mount::identity`]).
    #[must_use]
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = Some(identity.into());
        self
    }
}

/// Resource limits for one run. Every value must be non-zero.
///
/// The limits are *per run*, but their enforcement granularity differs: see the "Resource limits" table in
/// `docs/architecture/sandbox.md`. In short, `wall_ms` and `output_bytes` bound the whole sandbox,
/// `memory_bytes` is sampled over the whole namespace sandbox (and exact under cgroup v2 or an OCI runtime),
/// and the rest are per-process kernel limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Wall-clock budget in milliseconds, counted from the moment the sandbox is created
    /// (so it includes the few milliseconds of setup). Exceeding it kills everything.
    pub wall_ms: u64,
    /// CPU time per process in seconds (`RLIMIT_CPU`).
    pub cpu_s: u64,
    /// Memory in bytes. See [`Limits`] for how strictly each sandbox enforces it.
    pub memory_bytes: u64,
    /// Maximum number of tasks (processes **and threads**) the command may have, its main process included.
    pub pids: u64,
    /// Maximum number of open file descriptors per process (`RLIMIT_NOFILE`).
    pub open_files: u64,
    /// Maximum size of any one file the command writes (`RLIMIT_FSIZE`). Exceeding it raises `SIGXFSZ`.
    pub file_size_bytes: u64,
    /// Maximum bytes captured from **each** of stdout and stderr. The first byte over the limit kills the
    /// sandbox ([`crate::ExitStatus::OutputLimit`]).
    pub output_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            wall_ms: 60_000,
            cpu_s: 60,
            memory_bytes: 1 << 30,
            pids: 256,
            open_files: 1024,
            file_size_bytes: 1 << 30,
            output_bytes: 1 << 20,
        }
    }
}

impl Limits {
    pub(crate) fn validate(&self) -> Result<(), SandboxError> {
        let fields = [
            ("wall_ms", self.wall_ms),
            ("cpu_s", self.cpu_s),
            ("memory_bytes", self.memory_bytes),
            ("pids", self.pids),
            ("open_files", self.open_files),
            ("file_size_bytes", self.file_size_bytes),
            ("output_bytes", self.output_bytes),
        ];
        for (name, v) in fields {
            if v == 0 {
                return Err(SandboxError::invalid(format!("limit `{name}` must be greater than zero")));
            }
        }
        // rlimit values are `rlim_t` (unsigned long); u64 values beyond i64::MAX also confuse the
        // signed arithmetic of some container runtimes. Cap at a value no real limit approaches.
        for (name, v) in fields {
            if v > i64::MAX as u64 / 2 {
                return Err(SandboxError::invalid(format!("limit `{name}` is unreasonably large")));
            }
        }
        Ok(())
    }
}

/// Everything a sandbox needs to run one command.
///
/// The command sees exactly: the environment in [`env`](Self::env) (nothing is inherited), the tree in
/// [`workdir`](Self::workdir) mounted read-write at [`/work`](WORK_DIR) (also its working directory),
/// the [`ro_mounts`](Self::ro_mounts), a size-capped writable `/tmp`, and stdin from [`stdin`](Self::stdin).
/// The same spec can be given to any [`crate::Sandbox`]; how much of it is *isolated* depends on the
/// sandbox ([`crate::IsolationLevel`]).
///
/// To keep a spec portable, use paths relative to the working directory in `argv`; only
/// [`crate::ProcessSandbox`] runs in the host path of `workdir` instead of `/work`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxSpec {
    /// The command and its arguments. `argv[0]` is looked up on the `PATH` given in [`env`](Self::env)
    /// (default `/usr/local/bin:/usr/bin:/bin`) unless it contains a `/`.
    pub argv: Vec<String>,
    /// The complete environment of the command. **Nothing is inherited from the host.** Keep secrets out.
    pub env: BTreeMap<String, String>,
    /// Host directory holding the materialized tree. Mounted read-write at `/work`, which is the
    /// command's working directory. This is the only place the command can leave results on the host.
    pub workdir: PathBuf,
    /// Read-only host paths to expose (toolchains, system libraries).
    pub ro_mounts: Vec<Mount>,
    /// Size in bytes of the in-memory writable `/tmp` (counted against host memory while in use).
    pub rw_tmp_bytes: u64,
    /// Resource limits.
    pub limits: Limits,
    /// Network access. Defaults to [`Network::None`].
    pub network: Network,
    /// Bytes delivered on the command's standard input (then end-of-file).
    pub stdin: Vec<u8>,
}

impl SandboxSpec {
    /// A spec running `argv` with `workdir` as `/work`; empty environment, no extra mounts, default limits,
    /// no network, a 256 MiB `/tmp`, empty stdin.
    pub fn new<I, S>(argv: I, workdir: impl Into<PathBuf>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        SandboxSpec {
            argv: argv.into_iter().map(Into::into).collect(),
            env: BTreeMap::new(),
            workdir: workdir.into(),
            ro_mounts: Vec::new(),
            rw_tmp_bytes: 256 << 20,
            limits: Limits::default(),
            network: Network::None,
            stdin: Vec::new(),
        }
    }

    /// Set one environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Add a read-only mount.
    #[must_use]
    pub fn ro_mount(mut self, mount: Mount) -> Self {
        self.ro_mounts.push(mount);
        self
    }

    /// Add several read-only mounts.
    #[must_use]
    pub fn ro_mounts(mut self, mounts: impl IntoIterator<Item = Mount>) -> Self {
        self.ro_mounts.extend(mounts);
        self
    }

    /// Set the limits.
    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Set the network mode.
    #[must_use]
    pub fn network(mut self, network: Network) -> Self {
        self.network = network;
        self
    }

    /// Set the bytes delivered on stdin.
    #[must_use]
    pub fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = stdin.into();
        self
    }

    /// Set the size of the writable `/tmp`.
    #[must_use]
    pub fn rw_tmp_bytes(mut self, bytes: u64) -> Self {
        self.rw_tmp_bytes = bytes;
        self
    }

    /// Check the spec for mistakes that would otherwise surface half-way through sandbox setup.
    ///
    /// Every [`crate::Sandbox::run`] calls this first; calling it earlier lets a service reject a bad job
    /// before queueing it. It touches the file system only to check that `workdir` and the host side of
    /// each mount exist.
    pub fn validate(&self) -> Result<(), SandboxError> {
        if self.argv.is_empty() || self.argv[0].is_empty() {
            return Err(SandboxError::invalid("argv must name a program"));
        }
        if self.argv.iter().any(|a| a.contains('\0')) {
            return Err(SandboxError::invalid("argv must not contain NUL bytes"));
        }
        for (k, v) in &self.env {
            if k.is_empty() || k.contains(['=', '\0']) {
                return Err(SandboxError::invalid(format!("bad environment variable name {k:?}")));
            }
            if v.contains('\0') {
                return Err(SandboxError::invalid(format!("environment variable {k} contains a NUL byte")));
            }
        }
        self.limits.validate()?;
        if self.rw_tmp_bytes == 0 || self.rw_tmp_bytes > i64::MAX as u64 / 2 {
            return Err(SandboxError::invalid("rw_tmp_bytes must be greater than zero (and sane)"));
        }
        if !self.workdir.is_absolute() {
            return Err(SandboxError::invalid(format!("workdir {} must be an absolute path", self.workdir.display())));
        }
        if !self.workdir.is_dir() {
            return Err(SandboxError::invalid(format!("workdir {} is not a directory", self.workdir.display())));
        }
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.ro_mounts {
            if !m.host.is_absolute() {
                return Err(SandboxError::invalid(format!("mount source {} must be absolute", m.host.display())));
            }
            if std::fs::metadata(&m.host).is_err() {
                return Err(SandboxError::invalid(format!("mount source {} does not exist", m.host.display())));
            }
            let target = normalize_sandbox_path(&m.sandbox)?;
            if RESERVED.iter().any(|r| target == Path::new(r) || (*r != "/" && target.starts_with(r))) {
                return Err(SandboxError::invalid(format!(
                    "mount target {} is reserved (the sandbox provides /, /proc, /dev, /tmp and /work itself)",
                    target.display()
                )));
            }
            if !seen.insert(target.clone()) {
                return Err(SandboxError::invalid(format!("mount target {} is used twice", target.display())));
            }
        }
        Ok(())
    }
}

/// Check that `p` is an absolute path without `.`/`..` components and return it unchanged
/// (trailing slashes removed).
pub(crate) fn normalize_sandbox_path(p: &Path) -> Result<PathBuf, SandboxError> {
    if !p.is_absolute() {
        return Err(SandboxError::invalid(format!("mount target {} must be absolute", p.display())));
    }
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => {
                if n.as_encoded_bytes().contains(&0) {
                    return Err(SandboxError::invalid("mount target contains a NUL byte"));
                }
                out.push(n)
            }
            _ => {
                return Err(SandboxError::invalid(format!(
                    "mount target {} must not contain `.` or `..` components",
                    p.display()
                )));
            }
        }
    }
    Ok(out)
}

/// The usual read-only system directories of a Linux host that exist on this machine: `/usr`, `/bin`,
/// `/sbin`, `/lib*`, `/etc/alternatives`, `/etc/ld.so.cache` and `/etc/ssl/certs`, each mounted at the same
/// path. A convenience for running ordinary tools (`sh`, `python3`, a compiler driver) from the host's
/// installation; they carry no [`Mount::identity`].
///
/// Hosts differ (merged `/usr`, symlinked `/bin`); only paths that exist are returned. Data directories
/// that commonly hold secrets (`/etc` as a whole, `/home`, `/root`, `/var`) are deliberately not included.
pub fn standard_system_mounts() -> Vec<Mount> {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib32",
        "/lib64",
        "/libx32",
        "/etc/alternatives",
        "/etc/ld.so.cache",
        "/etc/ssl/certs",
    ]
    .iter()
    .filter(|p| Path::new(p).exists())
    .map(Mount::same_path)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(dir: &Path) -> SandboxSpec {
        SandboxSpec::new(["true"], dir)
    }

    #[test]
    fn a_default_spec_is_valid_and_isolated_by_default() {
        let d = std::env::temp_dir();
        let s = spec(&d);
        s.validate().unwrap();
        assert_eq!(s.network, Network::None);
        assert!(s.env.is_empty());
    }

    #[test]
    fn rejects_malformed_specs() {
        let d = std::env::temp_dir();
        let bad = |s: SandboxSpec| s.validate().unwrap_err().to_string();
        assert!(bad(SandboxSpec::new(Vec::<String>::new(), &d)).contains("argv"));
        assert!(bad(SandboxSpec::new(["a\0b"], &d)).contains("NUL"));
        assert!(bad(spec(&d).env("A=B", "x")).contains("environment variable name"));
        assert!(bad(spec(&d).env("A", "x\0")).contains("NUL"));
        assert!(bad(SandboxSpec::new(["true"], "relative/dir")).contains("absolute"));
        assert!(bad(SandboxSpec::new(["true"], "/definitely/not/here")).contains("not a directory"));
        assert!(bad(spec(&d).rw_tmp_bytes(0)).contains("rw_tmp_bytes"));
        let zero = Limits { pids: 0, ..Limits::default() };
        assert!(bad(spec(&d).limits(zero)).contains("pids"));
    }

    #[test]
    fn rejects_reserved_duplicate_and_unnormalized_mount_targets() {
        let d = std::env::temp_dir();
        let bad = |m: Vec<Mount>| spec(&d).ro_mounts(m).validate().unwrap_err().to_string();
        for target in ["/", "/proc", "/dev", "/dev/shm", "/tmp", "/tmp/x", "/work", "/work/sub", "/oldroot"] {
            assert!(bad(vec![Mount::new("/usr", target)]).contains("reserved"), "{target}");
        }
        assert!(bad(vec![Mount::new("/usr", "usr")]).contains("absolute"));
        assert!(bad(vec![Mount::new("/usr", "/a/../b")]).contains("`..`"));
        assert!(bad(vec![Mount::new("/usr", "/a"), Mount::new("/usr", "/a/")]).contains("twice"));
        assert!(bad(vec![Mount::new("/no/such/host/path", "/a")]).contains("does not exist"));
        assert!(bad(vec![Mount::new("usr", "/a")]).contains("absolute"));
        // Similar names are not reserved.
        spec(&d).ro_mounts(vec![Mount::new("/usr", "/tmpfs"), Mount::new("/usr", "/workbench")]).validate().unwrap();
    }

    #[test]
    fn the_standard_mounts_are_valid_for_this_host() {
        let d = std::env::temp_dir();
        spec(&d).ro_mounts(standard_system_mounts()).validate().unwrap();
    }

    #[test]
    fn limits_serialize_with_defaults_for_missing_fields() {
        let l: Limits = serde_json::from_str(r#"{"wall_ms": 5}"#).unwrap();
        assert_eq!(l.wall_ms, 5);
        assert_eq!(l.pids, Limits::default().pids);
    }
}
