use std::io;

/// Why a sandbox could not run a command.
///
/// These are *infrastructure* failures: the command did not get to run (or could not be started).
/// A command that ran and failed, timed out or hit a limit is a normal [`crate::Outcome`], not an error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SandboxError {
    /// The [`crate::SandboxSpec`] is malformed or asks for something this sandbox cannot do.
    #[error("invalid sandbox specification: {0}")]
    InvalidSpec(String),

    /// The sandbox mechanism cannot be used on this host (see [`crate::detect`] for the reason ahead of time).
    #[error("sandbox `{sandbox}` is unavailable on this host: {reason}")]
    Unavailable {
        /// Name of the sandbox, as returned by [`crate::Sandbox::name`].
        sandbox: &'static str,
        /// What is missing, with a hint on how to fix it where one is known.
        reason: String,
    },

    /// Setting up the isolated environment failed part-way. Nothing of the command ran.
    #[error("sandbox setup failed at `{stage}`: {source}")]
    Setup {
        /// The setup step that failed, e.g. `mount /work` or `pivot_root`.
        stage: String,
        /// The operating-system error.
        source: io::Error,
    },

    /// The program named by `argv[0]` could not be executed inside the sandbox
    /// (not found on the sandbox's `PATH`, not mounted, not executable).
    #[error("cannot execute `{program}` inside the sandbox: {source}")]
    Exec {
        /// `argv[0]` as given.
        program: String,
        /// The operating-system error (usually `ENOENT` or `EACCES`).
        source: io::Error,
    },

    /// An OCI runtime (`docker`/`podman`) reported its own failure, as opposed to the command's.
    #[error("container runtime failure: {0}")]
    Runtime(String),

    /// The caller asked for the run to stop ([`crate::Sandbox::run_cancellable`]); everything of the command was
    /// killed and cleaned up. No outcome exists.
    #[error("the run was cancelled")]
    Cancelled,

    /// The supervising side failed (pipes, `poll`, `wait`), or a killed sandbox did not die in time.
    #[error("sandbox supervision failed: {context}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// The operating-system error.
        source: io::Error,
    },
}

impl SandboxError {
    pub(crate) fn io(context: &'static str, source: io::Error) -> Self {
        SandboxError::Io { context, source }
    }

    pub(crate) fn setup(stage: impl Into<String>, source: io::Error) -> Self {
        SandboxError::Setup { stage: stage.into(), source }
    }

    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        SandboxError::InvalidSpec(msg.into())
    }
}
