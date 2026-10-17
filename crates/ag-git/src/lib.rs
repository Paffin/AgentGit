//! Git bridge: import a Git history into AgentGit and export a stream to Git.
//!
//! Git is an **external adapter** here, never the source of truth. The bridge
//! shells out to the `git` command line (plumbing commands only), so it works
//! with whatever Git you have installed and never links Git's code.
//!
//! # Mapping
//!
//! * A Git commit becomes a changeset with the same parents (same order). The
//!   operation list is the diff against the **first parent**, exactly the
//!   semantics of an AgentGit changeset, so merges carry over unchanged.
//! * Regular files, executable files and symlinks are preserved. File contents
//!   are stored as blobs.
//! * The author becomes `user:<email>` (sanitized); the commit time is kept; the
//!   original SHA is appended to the message as `Imported-From: git <sha>`.
//!
//! # Not carried over
//!
//! Submodules (gitlinks), paths AgentGit refuses to record (non-UTF-8, control
//! characters, backslashes, `.git`/`.ag` components), tags, notes, signatures,
//! committer identity and timezone offsets. Skipped paths are reported.

mod export;
mod import;
mod run;

pub use export::{ExportOptions, ExportReport, export};
pub use import::{ImportOptions, ImportReport, import};

/// Errors from the Git bridge.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// `git` could not be started.
    #[error("cannot run git: {0} (is git installed and on PATH?)")]
    Spawn(#[from] std::io::Error),
    /// `git` exited with an error.
    #[error("git {args} failed: {stderr}")]
    Failed {
        /// The arguments that were run.
        args: String,
        /// Its standard error output.
        stderr: String,
    },
    /// Output of `git` could not be understood.
    #[error("unexpected git output: {0}")]
    Parse(String),
    /// Engine failure.
    #[error(transparent)]
    Engine(#[from] ag_integrate::Error),
    /// The history cannot be represented.
    #[error("{0}")]
    Unsupported(String),
}
