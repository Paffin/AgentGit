import { AgentGitError } from './errors.js';
import { NAMESPACE_METHODS } from './generated/api.js';
import type { Api } from './generated/api.js';
import type { NegotiateResponse } from './generated/models.js';
import { OPERATIONS, PAGINATED } from './generated/operations.js';
import type { OperationId, PaginatedOperationId } from './generated/operations.js';
import type { StoredObject } from './changeset.js';
import { blobObject } from './hashing.js';
import { subscribeEvents } from './sse.js';
import {
  abortableSleep,
  DEFAULT_RETRY,
  DEFAULT_TIMEOUT_MS,
  Transport,
  type TokenSource,
  type TransportConfig,
} from './transport.js';
import type {
  AgentEvent,
  CallOptions,
  PutObjectResult,
  Req,
  Res,
  RetryOptions,
  SubscribeEventsOptions,
} from './types.js';

/** The protocol version this client speaks. */
export const PROTOCOL_VERSION = 'agwp.v1';

export interface AgentGitClientOptions {
  /** Server root, for example `https://git.example.com` (a path prefix behind a proxy is kept). */
  baseUrl: string;
  /** A bearer credential (`agt_...`, an OIDC token or the bootstrap token), or a function returning a fresh one per request. */
  token?: TokenSource;
  /** A `fetch` implementation (default: the global one). */
  fetch?: typeof fetch;
  /** Act as this agent run (`ag-run`): requests are then attributed to `agent:<name>#<run>`. */
  runId?: string;
  /** Deadline per attempt, sent as `Request-Timeout` and enforced locally (default 30 000 ms; `0` = none). */
  requestTimeoutMs?: number;
  /** Retry policy (see `RetryOptions`); `false` disables retries. */
  retry?: RetryOptions | false;
  /** Headers added to every request. */
  headers?: Record<string, string>;
  /** Replaces the sleep used between retries (tests). */
  sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
  /** Replaces the jitter source, a function returning a number in [0, 1) (tests). */
  random?: () => number;
}

/** The part of the response of `objects.putBlob`. */
export interface PutBlobResult extends PutObjectResult {
  /** The content id of the blob. */
  id: string;
}

type PaginatedKey<Id extends PaginatedOperationId> = (typeof PAGINATED)[Id];
/** The element type of a paginated operation's items. */
export type PageItem<Id extends PaginatedOperationId> =
  Res<Id> extends Record<PaginatedKey<Id>, readonly (infer I)[]> ? I : never;

export interface PaginateOptions extends CallOptions {
  /** Stop after this many items. */
  maxItems?: number;
}

/** Extra helpers next to the generated `objects` methods. */
interface ObjectsExtras {
  /** Uploads a blob (computing its id and envelope) and returns the id. */
  putBlob(request: { repo: string; content: Uint8Array | string }, options?: CallOptions): Promise<PutBlobResult>;
  /**
   * Uploads the objects the repository lacks (`objects.missing`, then `objects.put` for each), in the
   * order given - blobs before the changeset that uses them. Pass `[...built.blobs, built.object]`.
   */
  upload(request: { repo: string; objects: StoredObject[] }, options?: CallOptions): Promise<UploadResult>;
}

export interface UploadResult {
  /** Ids that were sent. */
  stored: string[];
  /** Ids the repository already had. */
  alreadyPresent: string[];
}

/** The client: one namespace per area of the API, one method per OpenAPI operation id. */
// eslint-disable-next-line @typescript-eslint/no-unsafe-declaration-merging
export interface AgentGitClient extends Omit<Api, 'objects'> {
  readonly objects: Api['objects'] & ObjectsExtras;
}

// eslint-disable-next-line @typescript-eslint/no-unsafe-declaration-merging
export class AgentGitClient {
  readonly #options: AgentGitClientOptions;
  readonly #transport: Transport;

  constructor(options: AgentGitClientOptions) {
    if (!options.baseUrl) throw new TypeError('AgentGitClient: baseUrl is required');
    const fetchFn = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (typeof fetchFn !== 'function') {
      throw new TypeError(
        'AgentGitClient: no global fetch found; pass the `fetch` option (Node 18+ and browsers have one)',
      );
    }
    this.#options = options;
    const config: TransportConfig = {
      baseUrl: options.baseUrl.replace(/\/+$/, ''),
      token: options.token,
      fetch: fetchFn,
      runId: options.runId,
      requestTimeoutMs: options.requestTimeoutMs ?? DEFAULT_TIMEOUT_MS,
      retry: { ...DEFAULT_RETRY, ...(options.retry === false ? { maxAttempts: 1 } : (options.retry ?? {})) },
      headers: options.headers ?? {},
      sleep: options.sleep ?? abortableSleep,
      random: options.random ?? Math.random,
    };
    this.#transport = new Transport(config);

    const self = this as unknown as Record<string, Record<string, unknown>>;
    for (const [namespace, methods] of Object.entries(NAMESPACE_METHODS)) {
      const bound: Record<string, unknown> = {};
      for (const [name, id] of Object.entries(methods)) {
        bound[name] = this.#bind(id);
      }
      self[namespace] = bound;
    }
    const objects = self['objects']!;
    objects['putBlob'] = async (
      request: { repo: string; content: Uint8Array | string },
      callOptions?: CallOptions,
    ): Promise<PutBlobResult> => {
      const { id, envelope } = blobObject(request.content);
      const result = (await this.#invoke(
        'objects.put',
        { repo: request.repo, object_id: id },
        envelope,
        callOptions,
      )) as PutObjectResult;
      return { ...result, id };
    };
    objects['upload'] = async (
      request: { repo: string; objects: StoredObject[] },
      callOptions?: CallOptions,
    ): Promise<UploadResult> => {
      const byId = new Map(request.objects.map((o) => [o.id, o] as const));
      const ids = [...byId.keys()];
      const missing = new Set<string>();
      // Ask in batches; the server tells which of the given ids it lacks.
      for (let i = 0; i < ids.length; i += 1000) {
        const answer = (await this.#invoke(
          'objects.missing',
          { repo: request.repo, object_ids: ids.slice(i, i + 1000) },
          undefined,
          callOptions,
        )) as { missing?: string[] };
        for (const id of answer.missing ?? []) missing.add(id);
      }
      const stored: string[] = [];
      for (const id of ids) {
        if (!missing.has(id)) continue;
        await this.#invoke('objects.put', { repo: request.repo, object_id: id }, byId.get(id)!.envelope, callOptions);
        stored.push(id);
      }
      return { stored, alreadyPresent: ids.filter((id) => !missing.has(id)) };
    };
    const events = self['events']!;
    events['subscribe'] = (opts: SubscribeEventsOptions) => this.subscribeEvents(opts);
  }

  /** The server root this client talks to. */
  get baseUrl(): string {
    return this.#transport.config.baseUrl;
  }

  #bind(id: OperationId): (request?: Record<string, unknown>, options?: CallOptions) => Promise<unknown> {
    const op = OPERATIONS[id];
    return (request, options) => {
      if (op.body === 'bytes') {
        const { data, ...rest } = (request ?? {}) as Record<string, unknown> & { data?: Uint8Array };
        if (!(data instanceof Uint8Array))
          throw new TypeError(`${op.namespace}.${op.name}: "data" must be a Uint8Array`);
        return this.#invoke(id, rest, data, options);
      }
      return this.#invoke(id, request ?? {}, undefined, options);
    };
  }

  #invoke(
    id: OperationId,
    request: Record<string, unknown>,
    data: Uint8Array | undefined,
    options: CallOptions = {},
  ): Promise<unknown> {
    return this.#transport.execute({ op: OPERATIONS[id], request, data, options });
  }

  /**
   * Calls any operation by its OpenAPI operation id. The namespaced methods (`client.proposals.create(...)`)
   * are the same thing with nicer names.
   */
  call<Id extends Exclude<OperationId, 'objects.put' | 'objects.get' | 'tree.file' | 'events.subscribe'>>(
    id: Id,
    request: Req<Id>,
    options?: CallOptions,
  ): Promise<Res<Id>> {
    return this.#invoke(id, request as Record<string, unknown>, undefined, options) as Promise<Res<Id>>;
  }

  /**
   * Asks the server what it supports and checks that it speaks `agwp.v1`. The client sends the versions it
   * speaks (`supported_versions=agwp.v1`); a server that speaks none of them, or one that answers with another
   * version, makes this throw a `failed_precondition` error.
   */
  async negotiate(
    info: { clientName?: string; clientVersion?: string } = {},
    options?: CallOptions,
  ): Promise<NegotiateResponse> {
    const request: Record<string, unknown> = { supported_versions: [PROTOCOL_VERSION] };
    if (info.clientName) request['client_name'] = info.clientName;
    if (info.clientVersion) request['client_version'] = info.clientVersion;
    const hello = (await this.#invoke('session.negotiate', request, undefined, options)) as NegotiateResponse;
    if (hello.protocol_version !== PROTOCOL_VERSION) {
      throw new AgentGitError({
        code: 'failed_precondition',
        message: `the server speaks ${hello.protocol_version || 'an unknown protocol'}, this client speaks ${PROTOCOL_VERSION}`,
        operation: 'session.negotiate',
      });
    }
    return hello;
  }

  /**
   * Iterates over every item of a paginated listing, following `next_page_token` until the last page:
   *
   * ```ts
   * for await (const proposal of client.paginate('proposal.list', { repo, state: 'open' })) { ... }
   * ```
   */
  async *paginate<Id extends PaginatedOperationId>(
    id: Id,
    request: Req<Id>,
    options: PaginateOptions = {},
  ): AsyncGenerator<PageItem<Id>, void, undefined> {
    const { maxItems, ...callOptions } = options;
    const key = PAGINATED[id];
    let seen = 0;
    for await (const page of this.pages(id, request, callOptions)) {
      for (const item of (page as unknown as Record<string, unknown[]>)[key] ?? []) {
        if (maxItems !== undefined && seen >= maxItems) return;
        seen++;
        yield item as PageItem<Id>;
      }
    }
  }

  /** Like `paginate`, but yields whole response pages. */
  async *pages<Id extends PaginatedOperationId>(
    id: Id,
    request: Req<Id>,
    options: CallOptions = {},
  ): AsyncGenerator<Res<Id>, void, undefined> {
    let token = ((request as Record<string, unknown>)['page_token'] as string | undefined) ?? '';
    for (;;) {
      const page = (await this.#invoke(
        id,
        { ...(request as Record<string, unknown>), page_token: token },
        undefined,
        options,
      )) as Res<Id>;
      yield page;
      const nextToken = (page as { next_page_token?: unknown }).next_page_token;
      const next = typeof nextToken === 'string' ? nextToken : '';
      if (next === '' || next === token) return;
      token = next;
    }
  }

  /**
   * Follows a repository's event feed over Server-Sent Events, resuming after the last delivered
   * `seq` whenever the connection drops. See `SubscribeEventsOptions`.
   */
  subscribeEvents(options: SubscribeEventsOptions): AsyncGenerator<AgentEvent, void, undefined> {
    return subscribeEvents(this.#transport, {
      ...(this.#options.runId && !options.runId ? { runId: this.#options.runId } : {}),
      ...options,
    });
  }

  /** A client that acts as the given agent run (every request carries `ag-run`). */
  withRun(runId: string): AgentGitClient {
    return new AgentGitClient({ ...this.#options, runId });
  }

  /** A client with another credential, everything else equal. */
  withToken(token: TokenSource): AgentGitClient {
    return new AgentGitClient({ ...this.#options, token });
  }
}
