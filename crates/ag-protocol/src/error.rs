//! The AGWP error model.
//!
//! Every failure carries a stable, machine-readable [`ErrorCode`]. The same
//! code appears as `ErrorInfo.code` in gRPC status details and in the HTTP
//! body `{"error": {...}}`, together with whether retrying can help.

use std::collections::BTreeMap;

use crate::v1::ErrorInfo;

/// Stable error codes (see `proto/agwp/v1/agwp.proto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// No or invalid credentials.
    Unauthenticated,
    /// Authenticated, but not allowed to do this.
    PermissionDenied,
    /// The entity does not exist (or the caller may not know it exists).
    NotFound,
    /// The entity already exists.
    AlreadyExists,
    /// The request is malformed or violates a documented constraint.
    InvalidArgument,
    /// The system is not in a state that allows the operation.
    FailedPrecondition,
    /// The stream moved since the caller read it; re-read and retry.
    StreamMoved,
    /// An idempotency key was reused with a different request.
    IdempotencyKeyReused,
    /// A referenced object has not been uploaded.
    ObjectMissing,
    /// An object failed hash verification.
    ObjectCorrupt,
    /// A size or count limit was exceeded.
    TooLarge,
    /// The caller is sending too fast; retry after the indicated delay.
    RateLimited,
    /// A quota was exhausted.
    QuotaExceeded,
    /// The server (or a dependency) is temporarily unavailable.
    Unavailable,
    /// The deadline expired. The operation may or may not have happened:
    /// ask for its outcome by operation id before retrying.
    DeadlineExceeded,
    /// Concurrent modification; retry.
    Aborted,
    /// An exclusive lease is held by someone else, or a fencing token is stale.
    LeaseConflict,
    /// The candidate no longer matches the tree a job was created for.
    StaleCandidate,
    /// The server does not implement this.
    Unimplemented,
    /// A bug or an unexpected failure on the server.
    Internal,
}

impl ErrorCode {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::PermissionDenied => "permission_denied",
            ErrorCode::NotFound => "not_found",
            ErrorCode::AlreadyExists => "already_exists",
            ErrorCode::InvalidArgument => "invalid_argument",
            ErrorCode::FailedPrecondition => "failed_precondition",
            ErrorCode::StreamMoved => "stream_moved",
            ErrorCode::IdempotencyKeyReused => "idempotency_key_reused",
            ErrorCode::ObjectMissing => "object_missing",
            ErrorCode::ObjectCorrupt => "object_corrupt",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::QuotaExceeded => "quota_exceeded",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::DeadlineExceeded => "deadline_exceeded",
            ErrorCode::Aborted => "aborted",
            ErrorCode::LeaseConflict => "lease_conflict",
            ErrorCode::StaleCandidate => "stale_candidate",
            ErrorCode::Unimplemented => "unimplemented",
            ErrorCode::Internal => "internal",
        }
    }

    /// Parses a wire string.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_str() == s)
    }

    /// Every code.
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::Unauthenticated,
        ErrorCode::PermissionDenied,
        ErrorCode::NotFound,
        ErrorCode::AlreadyExists,
        ErrorCode::InvalidArgument,
        ErrorCode::FailedPrecondition,
        ErrorCode::StreamMoved,
        ErrorCode::IdempotencyKeyReused,
        ErrorCode::ObjectMissing,
        ErrorCode::ObjectCorrupt,
        ErrorCode::TooLarge,
        ErrorCode::RateLimited,
        ErrorCode::QuotaExceeded,
        ErrorCode::Unavailable,
        ErrorCode::DeadlineExceeded,
        ErrorCode::Aborted,
        ErrorCode::LeaseConflict,
        ErrorCode::StaleCandidate,
        ErrorCode::Unimplemented,
        ErrorCode::Internal,
    ];

    /// HTTP status for this code.
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Unauthenticated => 401,
            ErrorCode::PermissionDenied => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::AlreadyExists => 409,
            ErrorCode::InvalidArgument => 400,
            ErrorCode::FailedPrecondition => 412,
            ErrorCode::StreamMoved => 409,
            ErrorCode::IdempotencyKeyReused => 422,
            ErrorCode::ObjectMissing => 422,
            ErrorCode::ObjectCorrupt => 422,
            ErrorCode::TooLarge => 413,
            ErrorCode::RateLimited => 429,
            ErrorCode::QuotaExceeded => 429,
            ErrorCode::Unavailable => 503,
            ErrorCode::DeadlineExceeded => 504,
            ErrorCode::Aborted => 409,
            ErrorCode::LeaseConflict => 409,
            ErrorCode::StaleCandidate => 409,
            ErrorCode::Unimplemented => 501,
            ErrorCode::Internal => 500,
        }
    }

    /// gRPC status code for this code.
    pub fn grpc_code(self) -> tonic::Code {
        use tonic::Code;
        match self {
            ErrorCode::Unauthenticated => Code::Unauthenticated,
            ErrorCode::PermissionDenied => Code::PermissionDenied,
            ErrorCode::NotFound => Code::NotFound,
            ErrorCode::AlreadyExists => Code::AlreadyExists,
            ErrorCode::InvalidArgument => Code::InvalidArgument,
            ErrorCode::FailedPrecondition => Code::FailedPrecondition,
            ErrorCode::StreamMoved => Code::Aborted,
            ErrorCode::IdempotencyKeyReused => Code::FailedPrecondition,
            ErrorCode::ObjectMissing => Code::FailedPrecondition,
            ErrorCode::ObjectCorrupt => Code::DataLoss,
            ErrorCode::TooLarge => Code::OutOfRange,
            ErrorCode::RateLimited => Code::ResourceExhausted,
            ErrorCode::QuotaExceeded => Code::ResourceExhausted,
            ErrorCode::Unavailable => Code::Unavailable,
            ErrorCode::DeadlineExceeded => Code::DeadlineExceeded,
            ErrorCode::Aborted => Code::Aborted,
            ErrorCode::LeaseConflict => Code::Aborted,
            ErrorCode::StaleCandidate => Code::Aborted,
            ErrorCode::Unimplemented => Code::Unimplemented,
            ErrorCode::Internal => Code::Internal,
        }
    }

    /// Whether retrying the same request (after re-reading state or waiting)
    /// can succeed.
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::StreamMoved
                | ErrorCode::RateLimited
                | ErrorCode::Unavailable
                | ErrorCode::Aborted
                | ErrorCode::DeadlineExceeded
        )
    }
}

/// An AGWP error: a code, a message and optional details.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ProtocolError {
    /// Stable code.
    pub code: ErrorCode,
    /// Human-readable message (safe to show to the caller).
    pub message: String,
    /// Suggested delay before retrying, when meaningful.
    pub retry_after_ms: i64,
    /// Request id for correlating with server logs.
    pub request_id: String,
    /// Structured details.
    pub details: BTreeMap<String, String>,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ProtocolError {
    /// Creates an error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), retry_after_ms: 0, request_id: String::new(), details: BTreeMap::new() }
    }

    /// Adds a detail.
    #[must_use]
    pub fn with_detail(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.details.insert(k.into(), v.into());
        self
    }

    /// Sets the retry delay.
    #[must_use]
    pub fn with_retry_after(mut self, ms: i64) -> Self {
        self.retry_after_ms = ms;
        self
    }

    /// Sets the request id.
    #[must_use]
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = id.into();
        self
    }

    /// The wire form.
    pub fn to_info(&self) -> ErrorInfo {
        ErrorInfo {
            code: self.code.as_str().to_string(),
            message: self.message.clone(),
            retryable: self.code.retryable(),
            retry_after_ms: self.retry_after_ms,
            request_id: self.request_id.clone(),
            details: self.details.clone(),
        }
    }

    /// Parses the wire form (unknown codes become `internal`).
    pub fn from_info(info: ErrorInfo) -> Self {
        Self {
            code: ErrorCode::parse(&info.code).unwrap_or(ErrorCode::Internal),
            message: info.message,
            retry_after_ms: info.retry_after_ms,
            request_id: info.request_id,
            details: info.details,
        }
    }

    /// Converts to a gRPC status carrying the `ErrorInfo` as its binary detail
    /// (`grpc-status-details-bin`) and the code in `ag-error-code` metadata.
    pub fn to_status(&self) -> tonic::Status {
        use prost::Message as _;
        let info = self.to_info();
        let mut status =
            tonic::Status::with_details(self.code.grpc_code(), self.message.clone(), info.encode_to_vec().into());
        if let Ok(v) = self.code.as_str().parse() {
            status.metadata_mut().insert("ag-error-code", v);
        }
        status
    }

    /// Recovers the AGWP error from a gRPC status (best effort).
    pub fn from_status(status: &tonic::Status) -> Self {
        use prost::Message as _;
        if let Ok(info) = ErrorInfo::decode(status.details())
            && !info.code.is_empty()
        {
            return Self::from_info(info);
        }
        let code =
            status.metadata().get("ag-error-code").and_then(|v| v.to_str().ok()).and_then(ErrorCode::parse).unwrap_or(
                match status.code() {
                    tonic::Code::Unauthenticated => ErrorCode::Unauthenticated,
                    tonic::Code::PermissionDenied => ErrorCode::PermissionDenied,
                    tonic::Code::NotFound => ErrorCode::NotFound,
                    tonic::Code::InvalidArgument => ErrorCode::InvalidArgument,
                    tonic::Code::Unavailable => ErrorCode::Unavailable,
                    tonic::Code::DeadlineExceeded => ErrorCode::DeadlineExceeded,
                    // How a client's own deadline surfaces (`tonic::TimeoutExpired`): the call was cut off because
                    // the server did not answer in time, which is a deadline and not an internal error.
                    tonic::Code::Cancelled if status.message() == "Timeout expired" => ErrorCode::DeadlineExceeded,
                    tonic::Code::ResourceExhausted => ErrorCode::RateLimited,
                    _ => ErrorCode::Internal,
                },
            );
        Self::new(code, status.message())
    }
}

impl From<ProtocolError> for tonic::Status {
    fn from(e: ProtocolError) -> Self {
        e.to_status()
    }
}
