/** The stable error codes of AGWP v1 (`ErrorInfo.code`). Unknown codes are passed through as strings. */
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
  | (string & {});

export interface AgentGitErrorInit {
  code: ErrorCode;
  message: string;
  /** HTTP status; `0` when no response was received (network failure, local timeout). */
  status?: number;
  retryable?: boolean;
  /** Server-suggested wait in milliseconds (`0` = none). */
  retryAfterMs?: number;
  /** Correlation id (`X-Request-Id`), also echoed by the server in every error. */
  requestId?: string;
  details?: Record<string, string>;
  /** The operation id, for example `proposal.create`. */
  operation?: string;
  /** How many attempts were made before this error was thrown. */
  attempts?: number;
  /**
   * The request may have been applied although no usable answer arrived (deadline, dropped
   * connection). Ask for the outcome (`operations.get`) before repeating a call that is not
   * idempotent.
   */
  outcomeUnknown?: boolean;
  cause?: unknown;
}

/** An error answered by the server (`{"error": ErrorInfo}`) or raised by the client for a transport failure. */
export class AgentGitError extends Error {
  override readonly name = 'AgentGitError';
  readonly code: ErrorCode;
  readonly status: number;
  /** Whether the server (or the client, for transport failures) considers a repeat worthwhile. */
  readonly retryable: boolean;
  readonly retryAfterMs: number;
  readonly requestId: string;
  readonly details: Record<string, string>;
  readonly operation: string | undefined;
  attempts: number;
  readonly outcomeUnknown: boolean;

  constructor(init: AgentGitErrorInit) {
    super(init.message, init.cause === undefined ? undefined : { cause: init.cause });
    this.code = init.code;
    this.status = init.status ?? 0;
    this.retryable = init.retryable ?? false;
    this.retryAfterMs = init.retryAfterMs ?? 0;
    this.requestId = init.requestId ?? '';
    this.details = init.details ?? {};
    this.operation = init.operation;
    this.attempts = init.attempts ?? 1;
    this.outcomeUnknown = init.outcomeUnknown ?? false;
    Object.setPrototypeOf(this, new.target.prototype);
  }

  /** `error.is('stream_moved', 'aborted')` - true when the code is one of the given ones. */
  is(...codes: ErrorCode[]): boolean {
    return codes.includes(this.code);
  }
}

/** Type guard that also works across duplicated copies of the package. */
export function isAgentGitError(value: unknown): value is AgentGitError {
  return value instanceof AgentGitError || (value instanceof Error && value.name === 'AgentGitError');
}

interface ErrorInfoWire {
  code?: string;
  message?: string;
  retryable?: boolean;
  retry_after_ms?: number;
  request_id?: string;
  details?: Record<string, string>;
}

/** Builds an error from the `ErrorInfo` of a response body. */
export function errorFromInfo(
  info: ErrorInfoWire,
  context: { status: number; operation?: string; requestId?: string; retryAfterMs?: number },
): AgentGitError {
  return new AgentGitError({
    code: info.code && info.code.length > 0 ? info.code : codeForStatus(context.status),
    message: info.message ?? `request failed with HTTP ${context.status}`,
    status: context.status,
    retryable: info.retryable ?? false,
    retryAfterMs: info.retry_after_ms && info.retry_after_ms > 0 ? info.retry_after_ms : (context.retryAfterMs ?? 0),
    requestId: info.request_id || context.requestId || '',
    details: info.details ?? {},
    operation: context.operation,
  });
}

/** The error for a response that is not the documented JSON envelope (proxies, 405, ...). */
export function errorFromStatus(
  status: number,
  body: string,
  context: { operation?: string; requestId?: string; retryAfterMs?: number },
): AgentGitError {
  const code = codeForStatus(status);
  const text = body.trim().slice(0, 200);
  return new AgentGitError({
    code,
    message: text ? `HTTP ${status}: ${text}` : `HTTP ${status}`,
    status,
    retryable: code === 'unavailable' || code === 'rate_limited' || code === 'deadline_exceeded',
    retryAfterMs: context.retryAfterMs ?? 0,
    requestId: context.requestId ?? '',
    operation: context.operation,
    outcomeUnknown: status >= 500,
  });
}

function codeForStatus(status: number): ErrorCode {
  switch (status) {
    case 400:
      return 'invalid_argument';
    case 401:
      return 'unauthenticated';
    case 403:
      return 'permission_denied';
    case 404:
      return 'not_found';
    case 412:
      return 'failed_precondition';
    case 413:
      return 'too_large';
    case 429:
      return 'rate_limited';
    case 501:
      return 'unimplemented';
    case 502:
    case 503:
      return 'unavailable';
    case 504:
      return 'deadline_exceeded';
    default:
      return status >= 500 ? 'internal' : 'http_error';
  }
}
