use ag_types::ObjectId;

/// Errors from the object store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An I/O error.
    #[error("object store I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A stored object failed verification.
    #[error("object {id} is corrupt: {reason}")]
    Corrupt {
        /// Identifier of the damaged object.
        id: ObjectId,
        /// What is wrong.
        reason: String,
    },
    /// The object exceeds a configured limit.
    #[error("object too large: {size} bytes exceeds limit {limit}")]
    TooLarge {
        /// Actual size.
        size: u64,
        /// Configured limit.
        limit: u64,
    },
    /// A remote object store refused a request or could not be reached, after
    /// the retries allowed for transient failures.
    #[error("{message}")]
    Remote {
        /// HTTP status of the last response, `None` if there was none (connection failure, timeout).
        status: Option<u16>,
        /// What happened, including the service's error code and message when it sent one.
        message: String,
    },
    /// The lock that coordinates writers with garbage collection could not be taken.
    #[error("object store lock error: {0}")]
    Lock(String),
}
