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
mod api_support;
mod backup;
mod blame;
mod comment_drafts;
mod derive;
mod directory;
mod draft_version;
mod error;
pub mod explain;
mod failpoints;
mod file_drafts;
mod fsck;
mod gc;
mod history;
mod integrate;
mod layout;
mod prepare;
mod publish;
mod queue;
mod repo;
mod review_drafts;
mod review_requests;
mod storage;
mod types;
mod virtual_ws;
pub mod workdir;
mod workspace;

pub use api_support::{FileDiff, MAX_NEGOTIATED};
pub use backup::BackupReport;
pub use blame::{Blame, BlameLine, BlameOptions, DEFAULT_BLAME_COMMITS, MAX_BLAME_FILE_SIZE, MAX_BLAME_LINES};
pub use comment_drafts::{CommentDraftInput, MAX_COMMENT_DRAFT_BYTES};
pub use derive::{DeriveReport, DeriveStatus, KEYFRAME_INTERVAL};
pub use directory::publish_new_directory;
pub use error::Error;
pub use failpoints::{Failpoints, GateHandle};
pub use file_drafts::{FileDraftInput, MAX_FILE_DRAFT_BYTES, MAX_FILE_DRAFTS};
pub use fsck::{FsckIssue, FsckReport};
pub use gc::{DEFAULT_GC_GRACE_MS, DEFAULT_REFLOG_RETENTION_MS, GcOptions, GcReport};
pub use history::{HybridGraph, LogEntry};
pub use integrate::ContentionPolicy;
pub use integrate::{IntegrateRequest, MergePreview, RecoveryReport};
pub use layout::{InitOptions, Layout, RepoConfig};
pub use prepare::{Preparation, PrepareContext, ValidationState};
pub use publish::PublishResult;
pub use review_drafts::{MAX_REVIEW_DRAFT_BYTES, ReviewDraftInput};

/// Lifecycle state of an operation (re-exported from the metadata store).
pub use ag_storage::meta::IntentState as PublishState;
pub use queue::QueueRunReport;
pub use repo::Repository;
pub use storage::{
    DEFAULT_PG_URL_ENV, DEFAULT_S3_ACCESS_KEY_ID_ENV, DEFAULT_S3_SECRET_ACCESS_KEY_ENV, DEFAULT_S3_SESSION_TOKEN_ENV,
    MAX_S3_LIST_PAGE, MAX_S3_REQUEST_TIMEOUT_SECS, MAX_S3_RETRIES, MetaConfig, ObjectLockKind, ObjectsConfig,
    StorageConfig,
};
pub use types::{
    Blocker, ConflictInfo, ConflictOriginInfo, IntegrationResult, NoteInfo, Outcome, ResolutionScopeInfo, Strategy,
};
pub use virtual_ws::{VirtualChange, VirtualContent};
pub use workspace::{Change, SyncOutcome};

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
