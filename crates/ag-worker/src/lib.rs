//! The execution-plane worker.
//!
//! When a stream's policy requires a check *and says how to run it*, `ag-server` queues a validation job for
//! the exact candidate tree. A worker with the `runner` role:
//!
//! 1. **claims** the job (long-poll, with a lease it keeps alive by heartbeat);
//! 2. fetches the candidate tree and **verifies** it is exactly the tree the job names (it recomputes the
//!    tree id from the manifest and hash-checks every blob — it does not take the server's word);
//! 3. writes the tree to a private directory and runs the policy's command on it **in a sandbox**
//!    ([`ag_sandbox`]) with the limits the policy asked for;
//! 4. **signs an attestation** — pass only for a clean exit with status 0 — with its own Ed25519 key, bound
//!    to the tree, the check, the policy version and the sandbox's environment digest;
//! 5. **completes** the job with the attestation, atomically.
//!
//! Whether the attestation *counts* is not the worker's decision: the policy lists the issuer identities
//! and public keys it trusts (`trusted_issuers`). A worker the policy does not list can run checks all day
//! and block nothing and unblock nothing.
//!
//! # What the worker keeps away from the check
//!
//! The signing key, the credential and the server connection live in the worker process; the sandbox gets
//! a fresh, empty file system containing the tree, a private `/tmp`, and read-only system directories — no
//! key, no token, no network (unless the policy asks and the worker allows). See
//! `docs/architecture/sandbox.md` for what each sandbox does and does not guarantee, and
//! `docs/guides/workers.md` for operating one.
//!
//! # Failure handling
//!
//! A *check* that fails is a signed `fail` attestation: the tree does not pass. A *worker* that cannot run
//! the check — the sandbox is unavailable, the toolchain is not mounted, the request exceeds what the worker
//! allows, the tree cannot be fetched — fails the **job** (with a retry delay), never signs a `fail`, and
//! lets another worker try. A candidate that has moved on completes the job as `stale`.

/// What the `ag-worker` binary says on a platform without the sandbox it drives (anything but Unix; the
/// namespace sandbox itself needs Linux). The worker is a hard "not supported" there — never a degraded mode
/// that runs a check without isolation.
pub const UNSUPPORTED_PLATFORM: &str = "the execution-plane worker is not supported on this platform: it runs \
checks in a sandbox built on Linux namespaces (or a container runtime), which Windows does not provide. \
Run `ag-worker` on a Linux host; `ag`, `ag-server` and the clients work on Windows. \
See docs/operations/windows.md.";

#[cfg(unix)]
mod attest;
#[cfg(unix)]
mod checkout;
#[cfg(unix)]
mod plan;
#[cfg(unix)]
mod profile;
#[cfg(unix)]
mod worker;

#[cfg(unix)]
pub use attest::{attestation_id, describe, result_json, tail, to_wire, verdict};
#[cfg(unix)]
pub use checkout::{Checkout, CheckoutError};
#[cfg(unix)]
pub use plan::{DEFAULT_PATH, Unrunnable, sandbox_spec};
#[cfg(unix)]
pub use profile::{Backend, Ceilings, DEFAULT_SANDBOX_ID, Defaults, Profile, Refusal};
#[cfg(unix)]
pub use worker::{Disposition, JobReport, Options, Worker};
