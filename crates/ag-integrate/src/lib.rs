//! The AgentGit engine: repositories, safe publication, integration.
//!
//! [`Repository`] ties the stores together and implements the guarantees of
//! the architecture:
//!
//! * **Atomic publication for readers.** A head becomes visible only after
//!   every object it needs was written, re-read and verified
//!   ([`Repository::publish`]); only then does one SQL transaction register
//!   the changesets.
//! * **Atomic integration.** [`Repository::integrate`] computes a candidate
//!   (fast-forward or deterministic merge), evaluates policy against facts
//!   read *inside* the transaction, and compare-and-swaps the stream head in
//!   that same transaction together with the reflog, audit and outbox rows.
//!   If the stream moved, the candidate is recomputed — never reused.
//! * **Idempotency and recovery.** Every mutation is an intent with an
//!   operation id and idempotency key; interrupted intents resume, and a
//!   client that lost a response asks [`Repository::operation`].
//! * **Safe garbage collection**, **integrity checking** and **rebuildable
//!   derived indexes**.

mod admin;
mod backup;
mod derive;
mod error;
mod failpoints;
mod fsck;
mod gc;
mod history;
mod integrate;
mod layout;
mod publish;
mod queue;
mod repo;
mod storage;
mod types;
pub mod workdir;
mod workspace;

pub use backup::BackupReport;
pub use derive::{DeriveReport, KEYFRAME_INTERVAL};
pub use error::Error;
pub use failpoints::Failpoints;
pub use fsck::{FsckIssue, FsckReport};
pub use gc::{GcOptions, GcReport};
pub use history::{HybridGraph, LogEntry};
pub use integrate::{IntegrateRequest, MergePreview, RecoveryReport};
pub use layout::{InitOptions, Layout, RepoConfig};
pub use publish::PublishResult;

/// Lifecycle state of an operation (re-exported from the metadata store).
pub use ag_storage::meta::IntentState as PublishState;
pub use queue::QueueRunReport;
pub use repo::Repository;
pub use storage::{
    DEFAULT_PG_URL_ENV, DEFAULT_S3_ACCESS_KEY_ID_ENV, DEFAULT_S3_SECRET_ACCESS_KEY_ENV, DEFAULT_S3_SESSION_TOKEN_ENV,
    MetaConfig, ObjectLockKind, ObjectsConfig, StorageConfig,
};
pub use types::{Blocker, ConflictInfo, IntegrationResult, Outcome, Strategy};
pub use workspace::{Change, SyncOutcome};

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
