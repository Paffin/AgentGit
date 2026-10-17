use ag_agent::AgentError;
use ag_changeset::ChangesetError;
use ag_dag::DagError;
use ag_policy::PolicyError;
use ag_storage::StoreError;
use ag_storage::meta::MetaError;
use ag_tree::TreeError;

/// Engine errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Object store failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Metadata store failure.
    #[error(transparent)]
    Meta(#[from] MetaError),
    /// Changeset encoding/validation failure.
    #[error(transparent)]
    Changeset(#[from] ChangesetError),
    /// Tree derivation failure.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// DAG traversal failure.
    #[error(transparent)]
    Dag(#[from] DagError),
    /// Policy document failure.
    #[error(transparent)]
    Policy(#[from] PolicyError),
    /// Agent layer failure.
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Cryptographic failure.
    #[error(transparent)]
    Crypto(#[from] ag_crypto::CryptoError),
    /// A referenced entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A caller-supplied value is invalid.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The request conflicts with the current state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Repository layout or configuration problem.
    #[error("repository error: {0}")]
    Config(String),
    /// A path would escape the working directory or follow a symlink.
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    /// The stream is too contended; retry later.
    #[error("too much contention: {0}")]
    Contention(String),
    /// A failure injected by tests.
    #[error("injected failure at '{0}'")]
    Injected(String),
}
