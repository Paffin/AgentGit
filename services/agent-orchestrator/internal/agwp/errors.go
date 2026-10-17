package agwp

import (
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"
)

// Error codes of AGWP v1 (docs/protocol/agwp-v1.md#errors). Clients must
// tolerate codes they do not know.
const (
	CodeUnauthenticated   = "unauthenticated"
	CodePermissionDenied  = "permission_denied"
	CodeNotFound          = "not_found"
	CodeAlreadyExists     = "already_exists"
	CodeInvalidArgument   = "invalid_argument"
	CodeFailedPrecond     = "failed_precondition"
	CodeStreamMoved       = "stream_moved"
	CodeIdempotencyReused = "idempotency_key_reused"
	CodeObjectMissing     = "object_missing"
	CodeObjectCorrupt     = "object_corrupt"
	CodeTooLarge          = "too_large"
	CodeRateLimited       = "rate_limited"
	CodeQuotaExceeded     = "quota_exceeded"
	CodeUnavailable       = "unavailable"
	CodeDeadlineExceeded  = "deadline_exceeded"
	CodeAborted           = "aborted"
	CodeLeaseConflict     = "lease_conflict"
	CodeStaleCandidate    = "stale_candidate"
	CodeUnimplemented     = "unimplemented"
	CodeInternal          = "internal"
)

// Error is a structured AGWP error as returned by the server.
type Error struct {
	// HTTPStatus is the status line of the response.
	HTTPStatus int
	// Code is the stable machine-readable code.
	Code string
	// Message is human text; never parse it.
	Message string
	// Retryable is the server's statement that repeating the request can help.
	Retryable bool
	// RetryAfter is the server's backoff hint (zero when none).
	RetryAfter time.Duration
	// RequestID correlates with server logs.
	RequestID string
	// Details carries code-specific context.
	Details map[string]string
}

// Error implements error.
func (e *Error) Error() string {
	var b strings.Builder
	fmt.Fprintf(&b, "agwp: %s", e.Code)
	if e.HTTPStatus != 0 {
		fmt.Fprintf(&b, " (HTTP %d)", e.HTTPStatus)
	}
	if e.Message != "" {
		fmt.Fprintf(&b, ": %s", e.Message)
	}
	if e.RequestID != "" {
		fmt.Fprintf(&b, " [request %s]", e.RequestID)
	}
	return b.String()
}

// AsError extracts a *Error from err's chain.
func AsError(err error) (*Error, bool) {
	var e *Error
	if errors.As(err, &e) {
		return e, true
	}
	return nil, false
}

// IsCode reports whether err is an AGWP error with one of the given codes.
func IsCode(err error, codes ...string) bool {
	e, ok := AsError(err)
	if !ok {
		return false
	}
	for _, c := range codes {
		if e.Code == c {
			return true
		}
	}
	return false
}

// IsNotFound reports whether err is a not_found error.
func IsNotFound(err error) bool { return IsCode(err, CodeNotFound) }

// IsRetryable reports whether err is an AGWP error the server marked retryable.
func IsRetryable(err error) bool {
	e, ok := AsError(err)
	return ok && e.Retryable
}

// ErrOutcomeUnknown marks a failed mutation that may or may not have been
// applied (the connection broke after the request was sent, or the deadline
// expired). Callers must look the state up before repeating the call.
var ErrOutcomeUnknown = errors.New("agwp: outcome unknown")

// OutcomeUnknownError wraps the transport failure of a mutation whose result
// the client cannot know.
type OutcomeUnknownError struct {
	Operation string
	Err       error
}

// Error implements error.
func (e *OutcomeUnknownError) Error() string {
	return fmt.Sprintf("agwp: %s: outcome unknown: %v", e.Operation, e.Err)
}

// Unwrap exposes both the cause and the ErrOutcomeUnknown sentinel.
func (e *OutcomeUnknownError) Unwrap() []error { return []error{e.Err, ErrOutcomeUnknown} }

// codeForStatus maps an HTTP status to the protocol code, used when a gateway
// or proxy answers with something that is not an AGWP error body.
func codeForStatus(status int) string {
	switch status {
	case http.StatusUnauthorized:
		return CodeUnauthenticated
	case http.StatusForbidden:
		return CodePermissionDenied
	case http.StatusNotFound:
		return CodeNotFound
	case http.StatusConflict:
		return CodeAborted
	case http.StatusBadRequest:
		return CodeInvalidArgument
	case http.StatusPreconditionFailed:
		return CodeFailedPrecond
	case http.StatusRequestEntityTooLarge:
		return CodeTooLarge
	case http.StatusTooManyRequests:
		return CodeRateLimited
	case http.StatusNotImplemented:
		return CodeUnimplemented
	case http.StatusServiceUnavailable, http.StatusBadGateway:
		return CodeUnavailable
	case http.StatusGatewayTimeout:
		return CodeDeadlineExceeded
	default:
		if status >= 500 {
			return CodeInternal
		}
		return CodeInvalidArgument
	}
}

// retryableStatus is the fallback when the body carries no "retryable" flag.
func retryableStatus(status int) bool {
	return status == http.StatusTooManyRequests || status == http.StatusServiceUnavailable || status == http.StatusBadGateway
}
