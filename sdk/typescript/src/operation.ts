/**
 * How a request is safe to repeat:
 * - `read`: it only reads;
 * - `natural`: repeating it has the same effect as making it once;
 * - `keyed`: the server records the outcome under the `Idempotency-Key` header;
 * - `unsafe`: a repeat may be applied twice, so it is never retried automatically.
 */
export type RetryClass = 'read' | 'natural' | 'keyed' | 'unsafe';

/** Static description of one HTTP operation (generated from the OpenAPI document). */
export interface OperationInfo {
  readonly id: string;
  readonly http: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  /** Route template, e.g. `/v1/repos/{owner}/{rname}/proposals/{id}`. */
  readonly route: string;
  readonly namespace: string;
  readonly name: string;
  readonly summary: string;
  readonly pathParams: readonly string[];
  readonly queryParams: readonly string[];
  readonly body: 'json' | 'bytes' | 'none';
  readonly response: 'json' | 'bytes' | 'sse';
  readonly retry: RetryClass;
  /** `natural` only when the request carries this field (e.g. an optimistic-concurrency guard). */
  readonly retryWhenField?: string;
  /** Whether the operation accepts an idempotency key (message field or HTTP header). */
  readonly idempotencyKey: boolean;
  /** Needs no credentials. */
  readonly public: boolean;
}
