//! What a worker is willing and able to do: which sandbox, which limits, which mounts.
//!
//! The *policy* says how a check is run (`required_validations[].run`); the *profile* says what this
//! particular machine allows. A job whose request exceeds the profile is **refused**, not silently trimmed:
//! running a check with less memory or time than the policy asked for would produce an attestation that
//! says something the policy did not ask for.

use std::path::PathBuf;

use ag_sandbox::{IsolationLevel, Mount, NamespaceSandbox, OciConfig, OciSandbox, ProcessSandbox, Sandbox};

/// The user and group the sandboxed command runs as on the host when the worker itself is root.
pub const DEFAULT_SANDBOX_ID: u32 = 65534;

/// The sandbox implementation a worker uses.
#[derive(Clone, Debug)]
pub enum Backend {
    /// Rootless namespaces. As root, the command is mapped to `host_ids` (see
    /// [`NamespaceSandbox::with_host_ids`]).
    Namespace {
        /// Host user and group for the command when the worker runs as root; ignored otherwise.
        host_ids: (u32, u32),
    },
    /// A container runtime. The image comes from the job's `run.image`, else `default_image`.
    Oci {
        /// `docker`, `podman` or a path.
        runtime: String,
        /// Image used when the policy names none (must be pinned by digest).
        default_image: Option<String>,
        /// `--user` for the container, `uid:gid`.
        user: String,
    },
    /// Limits only. **Not isolation**; refused unless the profile's `min_isolation` is `None`.
    Process,
}

/// A request the worker will not carry out.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Refusal(pub String);

impl Backend {
    /// The isolation level this backend provides.
    pub fn isolation(&self) -> IsolationLevel {
        match self {
            Backend::Namespace { .. } => IsolationLevel::Namespaces,
            Backend::Oci { .. } => IsolationLevel::Container,
            Backend::Process => IsolationLevel::None,
        }
    }

    /// Whether this backend can run a job that names a container image.
    pub fn supports_images(&self) -> bool {
        matches!(self, Backend::Oci { .. })
    }

    /// The sandbox for one job. `image` is the policy's `run.image`, if any.
    pub fn sandbox_for(&self, image: Option<&str>) -> Result<Box<dyn Sandbox>, Refusal> {
        match self {
            Backend::Namespace { host_ids } => {
                if let Some(image) = image {
                    return Err(Refusal(format!(
                        "the policy asks for image '{image}', which needs a container runtime; this worker uses the namespace sandbox"
                    )));
                }
                let (euid, _) = ag_sandbox::effective_ids();
                let sb = if euid == 0 {
                    NamespaceSandbox::new().with_host_ids(host_ids.0, host_ids.1)
                } else {
                    NamespaceSandbox::new()
                };
                Ok(Box::new(sb))
            }
            Backend::Process => {
                if let Some(image) = image {
                    return Err(Refusal(format!(
                        "the policy asks for image '{image}', which needs a container runtime; this worker uses the process sandbox"
                    )));
                }
                Ok(Box::new(ProcessSandbox::new()))
            }
            Backend::Oci { runtime, default_image, user } => {
                let image = image.or(default_image.as_deref()).ok_or_else(|| {
                    Refusal("the policy names no image and this worker has no default image".to_string())
                })?;
                let mut cfg = OciConfig::new(runtime.clone(), image.to_string());
                cfg.user = user.clone();
                let sb = OciSandbox::with_config(cfg).map_err(|e| Refusal(e.to_string()))?;
                Ok(Box::new(sb))
            }
        }
    }

    /// The host user and group that must own the checked-out tree for the command to be able to write it,
    /// when the worker runs as root. `None`: the worker's own ids are right.
    pub fn tree_owner(&self) -> Option<(u32, u32)> {
        let (euid, _) = ag_sandbox::effective_ids();
        if euid != 0 {
            return None;
        }
        match self {
            Backend::Namespace { host_ids } => Some(*host_ids),
            Backend::Oci { user, .. } => parse_numeric_user(user),
            Backend::Process => None,
        }
    }
}

fn parse_numeric_user(user: &str) -> Option<(u32, u32)> {
    let (u, g) = user.split_once(':').unwrap_or((user, user));
    Some((u.parse().ok()?, g.parse().ok()?))
}

/// The most a worker will grant a check. A request above any of these is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ceilings {
    /// Wall-clock seconds.
    pub timeout_secs: u32,
    /// Memory in MiB.
    pub memory_mb: u32,
    /// CPU seconds (per process).
    pub cpu_secs: u32,
    /// Processes and threads.
    pub max_pids: u32,
}

impl Default for Ceilings {
    fn default() -> Self {
        Ceilings { timeout_secs: 3600, memory_mb: 16 * 1024, cpu_secs: 4 * 3600, max_pids: 8192 }
    }
}

/// What a check gets when the policy leaves a limit at 0 ("worker default").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Defaults {
    /// Memory in MiB.
    pub memory_mb: u32,
    /// CPU seconds per process; `0` means "the wall-clock limit times the number of CPUs".
    pub cpu_secs: u32,
    /// Processes and threads.
    pub max_pids: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults { memory_mb: 2048, cpu_secs: 0, max_pids: 1024 }
    }
}

/// Everything about a worker that is not its identity or its connection.
#[derive(Clone, Debug)]
pub struct Profile {
    /// Which sandbox runs the checks.
    pub backend: Backend,
    /// The weakest isolation this worker will run a check under. The default is `Namespaces`; a worker
    /// that accepts `None` runs untrusted code with the privileges of its own user, and its attestations
    /// say so (`IsolationReport`), but it should only ever be used on code you already trust.
    pub min_isolation: IsolationLevel,
    /// Whether a policy may ask for `network: true` (the host's network, unrestricted).
    pub allow_network: bool,
    /// Read-only host paths mounted into every sandbox (system directories, toolchains).
    pub ro_mounts: Vec<Mount>,
    /// Where job directories are created.
    pub scratch_dir: PathBuf,
    /// Upper bounds.
    pub ceilings: Ceilings,
    /// Values for limits the policy left at 0.
    pub defaults: Defaults,
    /// Stdout and stderr are each capped at this many bytes (the run ends when exceeded).
    pub output_bytes: u64,
    /// The sandbox's `/tmp` size in MiB.
    pub tmp_mb: u64,
    /// How much of each output stream is kept in the job's result.
    pub tail_bytes: usize,
    /// The most the checked-out tree may weigh, in bytes.
    pub max_checkout_bytes: u64,
}

impl Profile {
    /// A profile using `backend` with conservative defaults, system directories mounted read-only, and job
    /// directories under the system temporary directory.
    pub fn new(backend: Backend) -> Self {
        Profile {
            min_isolation: IsolationLevel::Namespaces,
            backend,
            allow_network: false,
            ro_mounts: ag_sandbox::standard_system_mounts(),
            scratch_dir: std::env::temp_dir().join("ag-worker"),
            ceilings: Ceilings::default(),
            defaults: Defaults::default(),
            output_bytes: 8 << 20,
            tmp_mb: 512,
            tail_bytes: 8 * 1024,
            max_checkout_bytes: 4 << 30,
        }
    }

    /// A reason the command could not reach its working tree, if there is one.
    ///
    /// When the worker runs as root the command runs as another (unprivileged) user, and it has to be able to
    /// walk down to its working tree: every existing directory above `scratch_dir` must be searchable by
    /// others. (A directory created by the worker itself is; an existing `0700` one is the usual culprit.)
    pub fn scratch_problem(&self) -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        self.backend.tree_owner()?;
        let mut dir = self.scratch_dir.as_path();
        // Skip the part that does not exist yet: the worker creates it with the right mode.
        while !dir.exists() {
            dir = dir.parent()?;
        }
        loop {
            let mode = std::fs::metadata(dir).ok()?.permissions().mode();
            if mode & 0o001 == 0 {
                return Some(format!(
                    "{} (mode {:o}) cannot be searched by the unprivileged user the checks run as, so they could not reach their working tree; `chmod o+x` it or choose another --scratch-dir",
                    dir.display(),
                    mode & 0o7777
                ));
            }
            dir = dir.parent()?;
        }
    }

    /// Fails if the backend is weaker than `min_isolation`. Called at start-up so that a misconfigured
    /// worker refuses to start instead of refusing every job.
    pub fn check_isolation(&self) -> Result<(), Refusal> {
        if self.backend.isolation() < self.min_isolation {
            return Err(Refusal(format!(
                "the {:?} sandbox provides {:?} isolation, below the required {:?}",
                self.backend,
                self.backend.isolation(),
                self.min_isolation
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_isolation_floor_is_enforced() {
        let mut p = Profile::new(Backend::Process);
        assert!(p.check_isolation().is_err(), "the default floor refuses the process sandbox");
        p.min_isolation = IsolationLevel::None;
        assert!(p.check_isolation().is_ok());
        let p = Profile::new(Backend::Namespace { host_ids: (65534, 65534) });
        assert!(p.check_isolation().is_ok());
    }

    #[test]
    fn a_scratch_directory_the_check_could_not_reach_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut p = Profile::new(Backend::Namespace { host_ids: (65534, 65534) });
        p.scratch_dir = dir.path().join("not/yet/created");
        let (euid, _) = ag_sandbox::effective_ids();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        if euid == 0 {
            assert!(p.scratch_problem().is_some_and(|m| m.contains("cannot be searched")));
        } else {
            assert!(p.scratch_problem().is_none(), "only a root worker maps checks to another user");
        }
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // (the temp dir's own parents are searchable on any sane machine)
        assert!(p.scratch_problem().is_none());
    }

    #[test]
    fn images_need_a_container_runtime() {
        let ns = Backend::Namespace { host_ids: (1, 1) };
        assert!(ns.sandbox_for(Some("x@sha256:aa")).is_err());
        assert!(ns.sandbox_for(None).is_ok());
        let oci = Backend::Oci { runtime: "docker".into(), default_image: None, user: "65534:65534".into() };
        assert!(oci.sandbox_for(None).is_err(), "no image anywhere");
        let digest = format!("registry/img@sha256:{}", "ab".repeat(32));
        assert!(oci.sandbox_for(Some(&digest)).is_ok());
        assert!(oci.sandbox_for(Some("registry/img:latest")).is_err(), "unpinned images are refused by the sandbox");
    }

    #[test]
    fn numeric_container_users_are_understood() {
        assert_eq!(parse_numeric_user("65534:65533"), Some((65534, 65533)));
        assert_eq!(parse_numeric_user("1000"), Some((1000, 1000)));
        assert_eq!(parse_numeric_user("nobody"), None);
    }
}
