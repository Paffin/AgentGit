// The one error type the UI sees. It carries what the protocol promises (docs/protocol/agwp-v1.md "Errors").

export type ErrorCode =
  | 'unauthenticated'
  | 'permission_denied'
  | 'not_found'
  | 'already_exists'
  | 'invalid_argument'
  | 'failed_precondition'
  | 'stream_moved'
  | 'idempotency_key_reused'
  | 'object_missing'
  | 'object_corrupt'
  | 'too_large'
  | 'rate_limited'
  | 'quota_exceeded'
  | 'unavailable'
  | 'deadline_exceeded'
  | 'aborted'
  | 'lease_conflict'
  | 'stale_candidate'
  | 'unimplemented'
  | 'internal'
  // Client-side conditions (never sent by a server):
  | 'network_error'
  | 'client_timeout'
  | 'cancelled'
  | 'bad_response'
  | (string & {}) // servers may add codes; clients must tolerate unknown ones

export interface ApiErrorInit {
  status: number
  code: ErrorCode
  message: string
  retryable?: boolean
  retryAfterMs?: number
  requestId?: string
  details?: Record<string, string>
}

export class ApiError extends Error {
  readonly status: number
  readonly code: ErrorCode
  readonly retryable: boolean
  readonly retryAfterMs: number
  readonly requestId: string
  readonly details: Record<string, string>

  constructor(init: ApiErrorInit) {
    super(init.message)
    this.name = 'ApiError'
    this.status = init.status
    this.code = init.code
    this.retryable = init.retryable ?? false
    this.retryAfterMs = init.retryAfterMs ?? 0
    this.requestId = init.requestId ?? ''
    this.details = init.details ?? {}
  }

  get isUnauthenticated(): boolean {
    return this.code === 'unauthenticated'
  }
  get isForbidden(): boolean {
    return this.code === 'permission_denied'
  }
  get isNotFound(): boolean {
    return this.code === 'not_found'
  }
  /** The server could not be reached at all (as opposed to answering with an error). */
  get isNetwork(): boolean {
    return this.code === 'network_error' || this.code === 'client_timeout'
  }
  get isCancelled(): boolean {
    return this.code === 'cancelled'
  }
}

export function isApiError(e: unknown): e is ApiError {
  return e instanceof ApiError
}

const STATUS_CODES: Record<number, ErrorCode> = {
  400: 'invalid_argument',
  401: 'unauthenticated',
  403: 'permission_denied',
  404: 'not_found',
  409: 'aborted',
  412: 'failed_precondition',
  413: 'too_large',
  422: 'failed_precondition',
  429: 'rate_limited',
  500: 'internal',
  501: 'unimplemented',
  502: 'unavailable',
  503: 'unavailable',
  504: 'deadline_exceeded',
}

const RETRYABLE_STATUS = new Set([429, 502, 503, 504])

/** Parses `Retry-After` (delta-seconds or an HTTP date) into milliseconds. */
export function parseRetryAfter(value: string | null, now = Date.now()): number {
  if (!value) return 0
  const trimmed = value.trim()
  if (/^\d+$/.test(trimmed)) return Number(trimmed) * 1000
  const at = Date.parse(trimmed)
  return Number.isNaN(at) ? 0 : Math.max(0, at - now)
}

/**
 * Builds an ApiError from an HTTP response. Servers answer `{"error": ErrorInfo}`, but proxies and
 * gateways in front of them answer anything (HTML, empty bodies), so this never trusts the shape.
 */
export function errorFromResponse(
  status: number,
  body: unknown,
  headers: Headers,
  now = Date.now(),
): ApiError {
  const info =
    body &&
    typeof body === 'object' &&
    'error' in body &&
    body.error &&
    typeof body.error === 'object'
      ? (body.error as Record<string, unknown>)
      : null
  const headerRetry = parseRetryAfter(headers.get('retry-after'), now)
  const requestId = headers.get('x-request-id') ?? ''
  if (info) {
    const bodyRetry = typeof info.retry_after_ms === 'number' ? info.retry_after_ms : 0
    const details: Record<string, string> = {}
    if (info.details && typeof info.details === 'object') {
      for (const [k, v] of Object.entries(info.details)) details[k] = String(v)
    }
    return new ApiError({
      status,
      code:
        typeof info.code === 'string' && info.code
          ? info.code
          : (STATUS_CODES[status] ?? 'internal'),
      message: typeof info.message === 'string' && info.message ? info.message : `HTTP ${status}`,
      retryable:
        typeof info.retryable === 'boolean' ? info.retryable : RETRYABLE_STATUS.has(status),
      retryAfterMs: Math.max(bodyRetry, headerRetry),
      requestId: (typeof info.request_id === 'string' && info.request_id) || requestId,
      details,
    })
  }
  return new ApiError({
    status,
    code: STATUS_CODES[status] ?? (status >= 500 ? 'internal' : 'bad_response'),
    message:
      status === 0
        ? 'The server could not be reached'
        : `Unexpected response from the server (HTTP ${status})`,
    retryable: RETRYABLE_STATUS.has(status),
    retryAfterMs: headerRetry,
    requestId,
  })
}

/** Plain-language one-liner for an error, suitable for a toast or an inline alert. */
export function describeError(e: unknown): string {
  if (e instanceof ApiError) {
    switch (e.code) {
      case 'network_error':
        return 'Cannot reach the server. Check your connection; the page will retry on its own.'
      case 'client_timeout':
        return 'The server took too long to answer.'
      case 'unauthenticated':
        return 'Your credential is missing, invalid or expired.'
      case 'permission_denied':
        return e.message || 'You are not allowed to do that.'
      case 'rate_limited':
        return `Too many requests. Try again in ${Math.max(1, Math.ceil(e.retryAfterMs / 1000))}s.`
      default:
        return e.message
    }
  }
  if (e instanceof Error) return e.message
  return 'Something went wrong.'
}
