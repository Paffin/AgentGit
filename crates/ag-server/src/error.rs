//! Mapping from engine errors to AGWP errors.
//!
//! Anything the caller can act on keeps its message; unexpected failures are
//! logged with the request id and returned as an opaque `internal` error so
//! internals (paths, SQL, stack contents) never leak to clients.

use ag_agent::AgentError;
use ag_integrate::Error as EngineError;
use ag_protocol::{ErrorCode, ProtocolError};
use ag_storage::StoreError;
use ag_storage::meta::MetaError;

/// An error returned by the API layer.
#[derive(Debug, Clone)]
pub struct ApiError(pub ProtocolError);

/// Result alias of the API layer.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// Creates an error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(ProtocolError::new(code, message))
    }

    /// `invalid_argument`.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// `not_found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// `permission_denied`.
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    /// `internal`; the cause is logged, not returned.
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(error = %cause, "internal error");
        Self::new(ErrorCode::Internal, "internal error; see the server log for the request id")
    }

    /// The error code.
    pub fn code(&self) -> ErrorCode {
        self.0.code
    }

    /// Adds the request id.
    #[must_use]
    pub fn with_request_id(mut self, id: &str) -> Self {
        self.0.request_id = id.to_string();
        self
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ApiError {}

impl From<ProtocolError> for ApiError {
    fn from(e: ProtocolError) -> Self {
        Self(e)
    }
}

impl From<MetaError> for ApiError {
    fn from(e: MetaError) -> Self {
        match e {
            MetaError::NotFound(m) => Self::not_found(m),
            MetaError::AlreadyExists(m) => Self::new(ErrorCode::AlreadyExists, m),
            MetaError::CasMismatch { stream, expected, actual } => Self::new(
                ErrorCode::StreamMoved,
                format!("stream '{stream}' moved (expected version {expected}, now {actual})"),
            )
            .with_detail("stream", stream)
            .with_detail("expected_version", expected.to_string())
            .with_detail("actual_version", actual.to_string()),
            MetaError::IdempotencyKeyReused(k) => Self::new(
                ErrorCode::IdempotencyKeyReused,
                format!("idempotency key '{k}' was already used for a different request"),
            ),
            MetaError::InvalidTransition { from, to } => {
                Self::new(ErrorCode::FailedPrecondition, format!("invalid state transition {from} -> {to}"))
            }
            MetaError::ParentNotReady(id) => {
                Self::new(ErrorCode::ObjectMissing, format!("parent changeset {id} is not published"))
            }
            MetaError::Invalid(m) => Self::invalid(m),
            MetaError::UnsupportedSchema { .. } | MetaError::Corrupt(_) | MetaError::Json(_) => Self::internal(e),
            other => Self::internal(other),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Corrupt { id, reason } => {
                Self::new(ErrorCode::ObjectCorrupt, format!("object {id} is corrupt: {reason}"))
            }
            StoreError::TooLarge { size, limit } => {
                Self::new(ErrorCode::TooLarge, format!("object of {size} bytes exceeds the limit of {limit}"))
            }
            other => Self::internal(other),
        }
    }
}

impl From<AgentError> for ApiError {
    fn from(e: AgentError) -> Self {
        match e {
            AgentError::NotFound(m) => Self::not_found(m),
            AgentError::Invalid(m) => Self::invalid(m),
            AgentError::Conflict(m) => Self::new(ErrorCode::FailedPrecondition, m),
            AgentError::Verification(m) => Self::new(ErrorCode::InvalidArgument, m),
            AgentError::Meta(m) => m.into(),
            AgentError::Crypto(c) => Self::invalid(c.to_string()),
        }
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::NotFound(m) => Self::not_found(m),
            EngineError::Invalid(m) | EngineError::UnsafePath(m) => Self::invalid(m),
            EngineError::Conflict(m) => Self::new(ErrorCode::FailedPrecondition, m),
            EngineError::Contention(m) => Self::new(ErrorCode::Aborted, m).with_retry_after(200),
            EngineError::Meta(m) => m.into(),
            EngineError::Store(s) => s.into(),
            EngineError::Agent(a) => a.into(),
            EngineError::Policy(p) => Self::invalid(p.to_string()),
            EngineError::Changeset(c) => Self::invalid(c.to_string()),
            EngineError::Tree(t) => Self::invalid(t.to_string()),
            EngineError::Dag(d) => Self::invalid(d.to_string()),
            EngineError::Crypto(c) => Self::invalid(c.to_string()),
            EngineError::Json(j) => Self::invalid(j.to_string()),
            EngineError::Io(_) | EngineError::Config(_) | EngineError::Injected(_) => Self::internal(e),
        }
    }
}

impl From<ag_types::IdentityError> for ApiError {
    fn from(e: ag_types::IdentityError) -> Self {
        Self::invalid(e.to_string())
    }
}

impl ApiError {
    /// Adds a detail.
    #[must_use]
    pub fn with_detail(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.0 = self.0.with_detail(k, v);
        self
    }

    /// Sets a retry delay.
    #[must_use]
    pub fn with_retry_after(mut self, ms: i64) -> Self {
        self.0 = self.0.with_retry_after(ms);
        self
    }
}
