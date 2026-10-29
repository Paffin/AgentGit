import type { components, operations } from './generated/schema.js';
import type { OperationId } from './generated/operations.js';

/** Recursively optional: the wire protocol gives every missing field its zero value. */
export type DeepPartial<T> = T extends readonly (infer U)[]
  ? DeepPartial<U>[]
  : T extends object
    ? { [K in keyof T]?: DeepPartial<T[K]> }
    : T;

type Op<Id extends OperationId> = operations[Id];

type JsonBody<O> = O extends { requestBody: { content: { 'application/json': infer B } } } ? B : never;
type QueryOf<O> = O extends { parameters: { query?: infer Q } } ? Q : never;
type PathOf<O> = O extends { parameters: { path?: infer P } } ? P : never;

type OrEmpty<T> = [T] extends [never] ? unknown : T;
type BodyPart<O> = [JsonBody<O>] extends [never] ? unknown : DeepPartial<JsonBody<O>>;
type QueryPart<O> = OrEmpty<NonNullable<QueryOf<O>>>;
type PathPart<O> = [NonNullable<PathOf<O>>] extends [never] ? unknown : Omit<NonNullable<PathOf<O>>, 'owner' | 'rname'>;
type RepoPart<O> = O extends { parameters: { path: { owner: string; rname: string } } } ? { repo: string } : unknown;

/**
 * The request of an operation as one flat object, exactly like the protocol messages:
 * `repo` ("owner/name") instead of the two path segments, remaining path parameters, query
 * parameters and body fields side by side. Missing optional fields take the protocol default.
 */
export type Req<Id extends OperationId> = BodyPart<Op<Id>> & QueryPart<Op<Id>> & PathPart<Op<Id>> & RepoPart<Op<Id>>;

/** The decoded JSON response of an operation. */
export type Res<Id extends OperationId> =
  Op<Id> extends {
    responses: { 200: { content: { 'application/json': infer R } } };
  }
    ? R
    : never;

/** What `PUT /objects/{id}` answers (not described by the OpenAPI document). */
export interface PutObjectResult {
  object_id: string;
  /** `false` when the server already had the object. */
  stored: boolean;
}

/** A durable repository event (`seq` is strictly increasing per repository and never reused). */
export type AgentEvent = components['schemas']['Event'];

export interface RetryOptions {
  /** Total attempts including the first (default 4). `1` disables retries. */
  maxAttempts?: number;
  /** First backoff delay; doubles per attempt, with full jitter (default 200 ms). */
  baseDelayMs?: number;
  /** Cap of the backoff delay (default 10 000 ms). */
  maxDelayMs?: number;
  /**
   * A server-supplied `retry_after_ms` / `Retry-After` above this is not waited for: the error is
   * thrown instead (default 30 000 ms).
   */
  maxRetryAfterMs?: number;
  /**
   * Also retry operations the server cannot deduplicate (class `unsafe`). They may then be applied
   * twice. Default `false`.
   */
  retryUnsafe?: boolean;
}

export interface CallOptions {
  /** Abort the call (and any retries). */
  signal?: AbortSignal;
  /** Per-attempt deadline: sent as `Request-Timeout` and enforced locally. */
  timeoutMs?: number;
  /** Overrides the generated `Idempotency-Key` of a mutating call. */
  idempotencyKey?: string;
  /** Act as this agent run (`ag-run` header) for this call. */
  runId?: string;
  /** Extra request headers (for example `traceparent`). */
  headers?: Record<string, string>;
  /** Per-call retry settings; `false` disables retries. */
  retry?: RetryOptions | false;
  /** Overrides the correlation id (`X-Request-Id`). */
  requestId?: string;
}

export interface ReconnectOptions {
  /** Consecutive failed reconnects before giving up (default: unlimited). */
  maxAttempts?: number;
  baseDelayMs?: number;
  maxDelayMs?: number;
}

export interface SubscribeEventsOptions {
  /** "owner/name". */
  repo: string;
  /**
   * Deliver events with `seq` greater than this first. `0` replays the whole history; leave it out
   * to start from "now".
   */
  afterSeq?: number;
  /** Topic prefixes (`stream` matches `stream.integrate`); empty = all. */
  topics?: string[];
  /** Ask the server for a heartbeat this often (heartbeats are never yielded). */
  heartbeatMs?: number;
  /** Stop the subscription. The iterator then ends without an error. */
  signal?: AbortSignal;
  /** Reconnect (resuming after the last delivered `seq`) when the stream drops; `false` to never. */
  reconnect?: false | ReconnectOptions;
  /** Reconnect when nothing, not even a heartbeat, arrived for this long (default 90 000 ms). */
  idleTimeoutMs?: number;
  /** Act as this agent run. */
  runId?: string;
  headers?: Record<string, string>;
}

/** Explicit cross-repository feed. Checkpoint envelopes may have no event. */
export interface SubscribeManyEventsOptions extends Omit<SubscribeEventsOptions, 'repo' | 'afterSeq'> {
  /** Explicit repository set, between 1 and 30; held fixed across reconnects. */
  repos: string[];
  /** Opaque checkpoint from replayMany or subscribeMany; omit to start now. */
  cursor?: string;
}
