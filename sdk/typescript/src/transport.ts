import { AgentGitError, errorFromInfo, errorFromStatus, isAgentGitError } from './errors.js';
import { randomBytes } from './encoding.js';
import type { OperationInfo } from './operation.js';
import type { CallOptions, RetryOptions } from './types.js';

export type TokenSource = string | (() => string | Promise<string>);

export interface TransportConfig {
  baseUrl: string;
  token: TokenSource | undefined;
  fetch: typeof fetch;
  runId: string | undefined;
  requestTimeoutMs: number;
  retry: Required<RetryOptions>;
  headers: Record<string, string>;
  sleep: (ms: number, signal?: AbortSignal) => Promise<void>;
  random: () => number;
}

export const DEFAULT_RETRY: Required<RetryOptions> = {
  maxAttempts: 4,
  baseDelayMs: 200,
  maxDelayMs: 10_000,
  maxRetryAfterMs: 30_000,
  retryUnsafe: false,
};

export const DEFAULT_TIMEOUT_MS = 30_000;

/**
 * Errors the server marks retryable but where sending the identical request again cannot succeed:
 * `stream_moved` means a compare-and-swap lost against a newer version; the caller must re-read.
 */
const NEEDS_NEW_REQUEST = new Set<string>(['stream_moved']);

const URL_SAFE = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';

/** A random token made of `[A-Za-z0-9]`, valid as request id and idempotency key. */
export function randomToken(length: number): string {
  const bytes = randomBytes(length);
  let out = '';
  for (const b of bytes) out += URL_SAFE[b % URL_SAFE.length]!;
  return out;
}

function abortReason(signal: AbortSignal | undefined): Error {
  const reason: unknown = signal?.reason;
  return reason instanceof Error
    ? reason
    : new DOMException(typeof reason === 'string' ? reason : 'aborted', 'AbortError');
}

export function abortableSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortReason(signal));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(abortReason(signal));
    };
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/** The HTTP request derived from an operation and a flat request object. */
export interface BuiltRequest {
  url: string;
  method: string;
  /** JSON body text, when the operation takes one. */
  json: string | undefined;
  hasJsonBody: boolean;
}

const QUERY_CSV = new Set(['topics', 'repos']);

function encodeSegment(value: unknown, name: string, op: OperationInfo): string {
  if (value === undefined || value === null || value === '') {
    throw new TypeError(`${op.namespace}.${op.name}: missing required field "${name}"`);
  }
  return encodeURIComponent(scalar(value, `${op.namespace}.${op.name}: field "${name}"`));
}

/** Text of a primitive value; anything else cannot be a path segment or a query value. */
function scalar(value: unknown, what: string): string {
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') return String(value);
  throw new TypeError(`${what} must be a string, number or boolean`);
}

/** Maps a flat request object onto the route: path placeholders, query string or JSON body. */
export function buildRequest(op: OperationInfo, request: Record<string, unknown>, baseUrl: string): BuiltRequest {
  const used = new Set<string>();
  const repoScoped = op.pathParams.includes('owner') && op.pathParams.includes('rname');
  let repoParts: [string, string] | undefined;
  if (repoScoped) {
    const repo = request['repo'];
    const parts = typeof repo === 'string' ? repo.split('/') : [];
    if (parts.length !== 2 || !parts[0] || !parts[1]) {
      throw new TypeError(
        `${op.namespace}.${op.name}: "repo" must look like "owner/name", got ${JSON.stringify(repo)}`,
      );
    }
    repoParts = [parts[0], parts[1]];
    used.add('repo');
  }
  const path = op.route.replace(/\{(\*?)([A-Za-z_][A-Za-z0-9_]*)\}/g, (_m, wildcard: string, name: string) => {
    if (repoParts && name === 'owner') return encodeURIComponent(repoParts[0]);
    if (repoParts && name === 'rname') return encodeURIComponent(repoParts[1]);
    used.add(name);
    const value = request[name];
    if (wildcard) {
      const text = typeof value === 'string' ? value.replace(/^\/+/, '') : '';
      if (text === '') throw new TypeError(`${op.namespace}.${op.name}: missing required field "${name}"`);
      return text
        .split('/')
        .map((s) => encodeURIComponent(s))
        .join('/');
    }
    return encodeSegment(value, name, op);
  });

  const rest = Object.entries(request).filter(([k, v]) => !used.has(k) && v !== undefined);
  let url = baseUrl + path;
  let json: string | undefined;
  const hasJsonBody = op.body === 'json';
  if (hasJsonBody) {
    json = JSON.stringify(Object.fromEntries(rest));
  } else if ((op.body === 'none' || op.body === 'bytes') && rest.length > 0) {
    const query = new URLSearchParams();
    for (const [key, value] of rest) {
      if (value === null) continue;
      if (Array.isArray(value)) {
        if (QUERY_CSV.has(key)) query.set(key, value.map((v) => scalar(v, `field "${key}"`)).join(','));
        else for (const item of value) query.append(key, scalar(item, `field "${key}"`));
      } else if (typeof value === 'object') {
        throw new TypeError(`${op.namespace}.${op.name}: field "${key}" cannot be sent in a query string`);
      } else {
        query.append(key, scalar(value, `field "${key}"`));
      }
    }
    const qs = query.toString();
    if (qs) url += `?${qs}`;
  }
  return { url, method: op.http, json, hasJsonBody };
}

function parseRetryAfter(header: string | null): number {
  if (!header) return 0;
  const seconds = Number(header);
  if (Number.isFinite(seconds)) return Math.max(0, Math.round(seconds * 1000));
  const at = Date.parse(header);
  return Number.isNaN(at) ? 0 : Math.max(0, at - Date.now());
}

/** Everything the transport needs to know about one logical call. */
export interface PreparedCall {
  op: OperationInfo;
  request: Record<string, unknown>;
  data: Uint8Array | undefined;
  options: CallOptions;
}

export class Transport {
  constructor(readonly config: TransportConfig) {}

  /** Performs a call with retries and returns the decoded response (`json` object or `Uint8Array`). */
  async execute(call: PreparedCall): Promise<unknown> {
    const { op, request, options } = call;
    const built = buildRequest(op, request, this.config.baseUrl);
    const explicitKey =
      options.idempotencyKey ??
      (typeof request['idempotency_key'] === 'string' ? request['idempotency_key'] : undefined);
    const idempotencyKey =
      (built.hasJsonBody && op.idempotencyKey) || op.retry === 'keyed'
        ? explicitKey && explicitKey !== ''
          ? explicitKey
          : `sdk-${randomToken(24)}`
        : undefined;
    const requestId = options.requestId ?? `sdk-${randomToken(16)}`;
    const retry = this.retrySettings(options.retry);
    const allowUnsafe = retry.retryUnsafe;
    const retryClass = this.effectiveRetryClass(op, request);
    const timeoutMs = this.timeoutFor(op, request, options);

    let attempt = 0;
    for (;;) {
      attempt++;
      try {
        return await this.attempt(call, built, {
          idempotencyKey,
          requestId: attempt === 1 ? requestId : `${requestId}-r${attempt}`,
          timeoutMs,
        });
      } catch (e) {
        if (options.signal?.aborted) throw options.signal.reason ?? e;
        const err = isAgentGitError(e) ? e : this.wrapNetworkError(e, op);
        err.attempts = attempt;
        // A request the server rejected before running it (rate limit) can always be repeated;
        // anything else only if repeating it cannot apply it twice.
        const safe = retryClass !== 'unsafe' || allowUnsafe || (err.code === 'rate_limited' && err.status === 429);
        if (!(err.retryable && safe) || NEEDS_NEW_REQUEST.has(err.code) || attempt >= retry.maxAttempts) throw err;
        const wait = this.delayFor(err, attempt, retry);
        if (wait === undefined) throw err;
        await this.config.sleep(wait, options.signal);
      }
    }
  }

  /** Opens a streaming response (SSE); status errors are thrown like in `execute`. */
  async openStream(
    op: OperationInfo,
    request: Record<string, unknown>,
    options: { signal: AbortSignal; runId?: string | undefined; headers?: Record<string, string> | undefined },
  ): Promise<Response> {
    const built = buildRequest(op, request, this.config.baseUrl);
    const requestId = `sdk-${randomToken(16)}`;
    const headers = await this.headers(op, {
      requestId,
      idempotencyKey: undefined,
      timeoutMs: undefined,
      runId: options.runId,
      extra: options.headers,
    });
    headers.set('accept', 'text/event-stream');
    let response: Response;
    try {
      response = await this.config.fetch(built.url, { method: 'GET', headers, signal: options.signal });
    } catch (e) {
      if (options.signal.aborted) throw e;
      throw this.wrapNetworkError(e, op);
    }
    if (!response.ok) throw await this.errorFromResponse(response, op, requestId);
    return response;
  }

  private retrySettings(override: RetryOptions | false | undefined): Required<RetryOptions> {
    if (override === false) return { ...this.config.retry, maxAttempts: 1 };
    return { ...this.config.retry, ...stripUndefined(override ?? {}) };
  }

  private effectiveRetryClass(op: OperationInfo, request: Record<string, unknown>): OperationInfo['retry'] {
    if (op.retry === 'unsafe' && op.retryWhenField !== undefined) {
      // For example workspace.modify: with `expected_head` a second application is refused, never doubled.
      const guard = request[op.retryWhenField];
      if (guard !== undefined && guard !== null && guard !== '') return 'natural';
    }
    return op.retry;
  }

  private timeoutFor(op: OperationInfo, request: Record<string, unknown>, options: CallOptions): number {
    if (options.timeoutMs !== undefined) return options.timeoutMs;
    const wait = Number(request['wait_ms'] ?? 0);
    const base = this.config.requestTimeoutMs;
    return Number.isFinite(wait) && wait > 0 ? Math.max(base, wait + 10_000) : base;
  }

  private delayFor(err: AgentGitError, attempt: number, retry: Required<RetryOptions>): number | undefined {
    const { random } = this.config;
    // Full jitter over an exponentially growing window. A server-supplied delay is a floor, never
    // shortened: many callers that were told the same delay would otherwise all return at once.
    const ceiling = Math.min(retry.maxDelayMs, retry.baseDelayMs * 2 ** (attempt - 1));
    const jitter = random() * ceiling;
    if (err.retryAfterMs > 0) {
      if (err.retryAfterMs > retry.maxRetryAfterMs) return undefined;
      return Math.round(err.retryAfterMs + jitter);
    }
    return Math.round(jitter);
  }

  private wrapNetworkError(e: unknown, op: OperationInfo): AgentGitError {
    const reason = e instanceof Error ? e.message : String(e);
    const cause = e instanceof Error && e.cause instanceof Error ? `: ${e.cause.message}` : '';
    return new AgentGitError({
      code: 'unavailable',
      message: `${op.id}: network error (${reason}${cause})`,
      status: 0,
      retryable: true,
      operation: op.id,
      outcomeUnknown: true,
      cause: e,
    });
  }

  private async headers(
    op: OperationInfo,
    h: {
      requestId: string;
      idempotencyKey: string | undefined;
      timeoutMs: number | undefined;
      runId: string | undefined;
      extra: Record<string, string> | undefined;
    },
  ): Promise<Headers> {
    const headers = new Headers();
    headers.set('x-request-id', h.requestId);
    if (!op.public && this.config.token !== undefined) {
      const token = typeof this.config.token === 'function' ? await this.config.token() : this.config.token;
      if (token) headers.set('authorization', `Bearer ${token}`);
    }
    const run = h.runId ?? this.config.runId;
    if (run) headers.set('ag-run', run);
    if (h.timeoutMs !== undefined && h.timeoutMs > 0) headers.set('request-timeout', String(Math.ceil(h.timeoutMs)));
    if (h.idempotencyKey) headers.set('idempotency-key', h.idempotencyKey);
    for (const [k, v] of Object.entries(this.config.headers)) headers.set(k, v);
    for (const [k, v] of Object.entries(h.extra ?? {})) headers.set(k, v);
    return headers;
  }

  private async attempt(
    call: PreparedCall,
    built: BuiltRequest,
    ctx: { idempotencyKey: string | undefined; requestId: string; timeoutMs: number },
  ): Promise<unknown> {
    const { op, options, data } = call;
    const headers = await this.headers(op, {
      requestId: ctx.requestId,
      idempotencyKey: ctx.idempotencyKey,
      timeoutMs: ctx.timeoutMs,
      runId: options.runId,
      extra: options.headers,
    });
    let body: BodyInit | undefined;
    if (built.hasJsonBody) {
      headers.set('content-type', 'application/json');
      body = built.json;
    } else if (op.body === 'bytes') {
      headers.set('content-type', 'application/octet-stream');
      body = data as BodyInit;
    }
    headers.set('accept', op.response === 'bytes' ? 'application/octet-stream, application/json' : 'application/json');

    const controller = new AbortController();
    let timedOut = false;
    const grace = ctx.timeoutMs > 0 ? Math.min(1000, Math.max(100, Math.round(ctx.timeoutMs * 0.1))) : 0;
    const timer =
      ctx.timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true;
            controller.abort();
          }, ctx.timeoutMs + grace)
        : undefined;
    const onAbort = (): void => controller.abort(options.signal?.reason);
    if (options.signal?.aborted) onAbort();
    options.signal?.addEventListener('abort', onAbort, { once: true });
    try {
      let response: Response;
      try {
        response = await this.config.fetch(built.url, {
          method: built.method,
          headers,
          body,
          signal: controller.signal,
        });
      } catch (e) {
        if (timedOut) throw this.timeoutError(op, ctx.timeoutMs, ctx.requestId);
        throw e;
      }
      if (!response.ok) throw await this.errorFromResponse(response, op, ctx.requestId);
      try {
        if (op.response === 'bytes') return new Uint8Array(await response.arrayBuffer());
        const text = await response.text();
        return text === '' ? {} : (JSON.parse(text) as unknown);
      } catch (e) {
        if (timedOut) throw this.timeoutError(op, ctx.timeoutMs, ctx.requestId);
        if (isAgentGitError(e)) throw e;
        if (e instanceof SyntaxError) {
          throw new AgentGitError({
            code: 'internal',
            message: `${op.id}: the server answered ${response.status} with a body that is not JSON`,
            status: response.status,
            operation: op.id,
            requestId: response.headers.get('x-request-id') ?? ctx.requestId,
            cause: e,
          });
        }
        throw e;
      }
    } finally {
      if (timer !== undefined) clearTimeout(timer);
      options.signal?.removeEventListener('abort', onAbort);
    }
  }

  private timeoutError(op: OperationInfo, timeoutMs: number, requestId: string): AgentGitError {
    return new AgentGitError({
      code: 'deadline_exceeded',
      message: `${op.id}: no answer within ${timeoutMs} ms; the operation may still complete`,
      status: 0,
      retryable: true,
      requestId,
      operation: op.id,
      outcomeUnknown: true,
    });
  }

  private async errorFromResponse(response: Response, op: OperationInfo, requestId: string): Promise<AgentGitError> {
    const text = await response.text().catch(() => '');
    const retryAfterMs = parseRetryAfter(response.headers.get('retry-after'));
    const echoed = response.headers.get('x-request-id') ?? requestId;
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = undefined;
    }
    const info = (parsed as { error?: unknown } | undefined)?.error;
    if (info !== null && typeof info === 'object') {
      const err = errorFromInfo(info, {
        status: response.status,
        operation: op.id,
        requestId: echoed,
        retryAfterMs,
      });
      return withOutcome(err);
    }
    return errorFromStatus(response.status, text, { operation: op.id, requestId: echoed, retryAfterMs });
  }
}

function withOutcome(err: AgentGitError): AgentGitError {
  // The server marks a deadline as retryable but the operation may have completed.
  if (err.code === 'deadline_exceeded' && !err.outcomeUnknown) {
    return new AgentGitError({
      code: err.code,
      message: err.message,
      status: err.status,
      retryable: err.retryable,
      retryAfterMs: err.retryAfterMs,
      requestId: err.requestId,
      details: err.details,
      ...(err.operation === undefined ? {} : { operation: err.operation }),
      outcomeUnknown: true,
    });
  }
  return err;
}

function stripUndefined<T extends object>(o: T): Partial<T> {
  return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as Partial<T>;
}
