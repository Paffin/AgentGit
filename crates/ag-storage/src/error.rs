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
    /// A fault injected by the test harness.
    #[error("injected fault: {0}")]
    Injected(String),
}
