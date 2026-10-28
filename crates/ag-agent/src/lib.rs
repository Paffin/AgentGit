//! Agent-native coordination on top of the SCM.
//!
//! Everything here treats agents as **untrusted participants**: they get an
//! identity, may *propose* changes and *claim* work, but nothing in this
//! crate lets them bypass the SCM core or the policy engine.
//!
//! * [`registry`] — agent identities with Ed25519 keys, and runs;
//! * [`tasks`] — task ownership and dependency awareness;
//! * [`leases`] — soft leases (advisory signals) and exclusive leases
//!   enforced server-side with fencing tokens;
//! * [`provenance`] — signed records of who/why/with-what changed the code,
//!   stored apart from the immutable changeset;
//! * [`predict`] — early detection of overlapping work.

pub mod executor;
pub mod leases;
pub mod predict;
pub mod provenance;
pub mod registry;
pub mod scheduling;
pub mod tasks;

use ag_storage::meta::MetaError;

/// Errors from agent coordination.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Metadata store failure.
    #[error(transparent)]
    Meta(#[from] MetaError),
    /// Key handling failure.
    #[error(transparent)]
    Crypto(#[from] ag_crypto::CryptoError),
    /// A referenced entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// An argument is invalid.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The operation conflicts with existing state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A signature or record failed verification.
    #[error("verification failed: {0}")]
    Verification(String),
}
