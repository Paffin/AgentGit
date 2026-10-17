//! Isolated execution of untrusted commands.
//!
//! AgentGit binds a validation to the exact tree, policy version and *environment* it ran in
//! (ADR-0001 §8). This crate is how a trusted execution plane runs the check: it takes a materialized tree
//! and a command, runs the command with CPU, memory, process, file and output limits, no network, no
//! secrets and a private view of the file system, and reports exactly which protections were in force so
//! that an attestation can say what it is worth.
//!
//! Three implementations of [`Sandbox`] share one [`SandboxSpec`] and one [`Outcome`]:
//!
//! | | [`IsolationLevel`] | needs | use for |
//! |---|---|---|---|
//! | [`NamespaceSandbox`] | `Namespaces` | Linux, unprivileged user namespaces | the default execution plane |
//! | [`OciSandbox`] | `Container` | `docker` or `podman`, a pinned image | hostile or multi-tenant workloads |
//! | [`ProcessSandbox`] | `None` | any Unix | local runs of trusted code; **not isolation** |
//!
//! ```no_run
//! use ag_sandbox::{best_available, Limits, Mount, SandboxSpec, standard_system_mounts};
//!
//! let sandbox = best_available();
//! let spec = SandboxSpec::new(["sh", "-c", "cargo test"], "/srv/trees/abc123")
//!     .env("PATH", "/usr/bin:/bin")
//!     .ro_mounts(standard_system_mounts())
//!     .ro_mount(Mount::same_path("/opt/rust").with_identity("rust-1.89.0"))
//!     .limits(Limits { wall_ms: 600_000, ..Limits::default() });
//! let outcome = sandbox.run(&spec)?;
//! println!("{:?} under {} ({:?})", outcome.status, outcome.environment_digest, sandbox.isolation());
//! # Ok::<(), ag_sandbox::SandboxError>(())
//! ```
//!
//! # What this is not
//!
//! None of these is a virtual machine: they share the host kernel, and a kernel vulnerability reachable
//! from the permitted system calls is a way out. `docs/architecture/sandbox.md` states per mechanism what it
//! stops and what it does not, and why container or VM isolation is recommended for hostile multi-tenant use.
//! [`IsolationReport`] never claims more than was set up, and the integration tests read the kernel's view
//! from inside the sandbox to check it.
//!
//! # `unsafe`
//!
//! This is the only AgentGit crate that uses `unsafe` (the workspace forbids it elsewhere). It is denied
//! crate-wide and re-allowed in the three modules that make system calls; every block carries a `SAFETY`
//! comment, and the code that runs between `clone` and `exec` is restricted to async-signal-safe operations
//! (see the `spawn` module's documentation).
#![cfg(unix)]

mod cgroup;
mod detect;
mod digest;
mod error;
#[cfg(target_os = "linux")]
mod mem;
#[cfg(target_os = "linux")]
mod namespace;
#[cfg(not(target_os = "linux"))]
#[path = "namespace_stub.rs"]
mod namespace;
#[cfg(target_os = "linux")]
mod nsinit;
#[cfg(not(target_os = "linux"))]
#[path = "nsinit_stub.rs"]
mod nsinit;
mod oci;
mod outcome;
mod planning;
mod process;
#[cfg(target_os = "linux")]
mod seccomp;
mod spawn;
mod spec;
mod supervise;
mod sys;

pub use detect::{Availability, best_available, detect};
pub use digest::environment_digest;
pub use error::SandboxError;
pub use namespace::NamespaceSandbox;
pub use oci::{OciConfig, OciSandbox};
pub use outcome::{CgroupReport, Enforcement, ExitStatus, IsolationLevel, IsolationReport, Outcome};
pub use process::ProcessSandbox;
pub use spec::{Limits, Mount, Network, SandboxSpec, WORK_DIR, standard_system_mounts};

/// Something that can run a command in an environment it controls.
///
/// Implementations are cheap to share between threads and every [`run`](Sandbox::run) is independent: each
/// creates and tears down its own environment. `run` blocks the calling thread until nothing of the command
/// is left (or the limits ended it), so call it from a blocking context or a dedicated thread.
///
/// An `Err` is an infrastructure failure — the command did not run, or could not be supervised. A command
/// that ran and failed, timed out or hit a limit is an `Ok(Outcome)` whose [`Outcome::status`] says so.
pub trait Sandbox: Send + Sync {
    /// Short stable name: `"namespace"`, `"oci"` or `"process"`. Part of [`environment_digest`].
    fn name(&self) -> &'static str;

    /// How strongly this sandbox separates the command from the host.
    fn isolation(&self) -> IsolationLevel;

    /// Run `spec` to completion. Validates the spec first ([`SandboxSpec::validate`]).
    fn run(&self, spec: &SandboxSpec) -> Result<Outcome, SandboxError>;
}
