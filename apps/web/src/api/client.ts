// A small typed fetch client for AGWP over HTTP/JSON.
//
//  * `Authorization: Bearer …` comes from `getToken()` per request; the token is never logged or put in a URL.
//  * Every request carries `X-Request-Id`; mutations carry an `Idempotency-Key` (one per logical action,
//    reused across retries, so a lost response can never apply twice).
//  * `Request-Timeout` tells the server our deadline; the client aborts a little later.
//  * Only retryable failures are retried, honouring `Retry-After`. A mutation without an idempotency key is
//    only retried when the server proves it did nothing (429/503).
import { ApiError, errorFromResponse } from './errors'
import { observeServerDate } from './clock'

export type QueryValue = string | number | boolean | readonly string[] | null | undefined
export type Query = Record<string, QueryValue>
export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'

export interface RequestOptions {
  method?: HttpMethod
  query?: Query
  body?: unknown
  /** Binary upload; uses the same authentication, deadline and retry contract as JSON. */
  binary?: Blob
  signal?: AbortSignal
  /** Deadline sent as `Request-Timeout` (ms). 0 disables it. */
  timeoutMs?: number
  /** Explicit key, or `false` to send none. Default: a fresh key for every mutation. */
  idempotencyKey?: string | false
  /** Act as an agent run (`ag-run` header). */
  runId?: string
  headers?: Record<string, string>
  /** Override the client's retry budget for this call. */
  retries?: number
  /** Do not treat a 401 as "the session is gone" (used while checking a pasted token). */
  quietUnauthenticated?: boolean
  /** Use this token instead of the client's current one. */
  token?: string | null
  /** The response is a long-lived stream: no deadline once the headers arrived. */
  stream?: boolean
  /** A POST that only reads (diff, prepare): safe to retry after any failure, with no key needed. */
  readOnly?: boolean
}

export interface ClientOptions {
  baseUrl?: string
  getToken: () => string | null
  fetch?: typeof fetch
  sleep?: (ms: number) => Promise<void>
  random?: () => number
  now?: () => number
  /** Retries after the first attempt. */
  maxRetries?: number
  /** Longest wait between attempts we are willing to sit through (also caps honoured Retry-After). */
  maxRetryWaitMs?: number
  defaultTimeoutMs?: number
  onUnauthenticated?: (e: ApiError) => void
  newRequestId?: () => string
  newIdempotencyKey?: () => string
}

const MUTATING = new Set<HttpMethod>(['POST', 'PUT', 'PATCH', 'DELETE'])
const ABORT_GRACE_MS = 3000

/** Random bytes as hex; works in insecure contexts where `crypto.randomUUID` does not exist. */
export function randomHex(bytes = 16): string {
  const buf = new Uint8Array(bytes)
  globalThis.crypto.getRandomValues(buf)
  return Array.from(buf, (b) => b.toString(16).padStart(2, '0')).join('')
}

export function buildQuery(query: Query | undefined): string {
  if (!query) return ''
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null || value === '') continue
    params.set(
      key,
      typeof value === 'string' ? value : Array.isArray(value) ? value.join(',') : String(value),
    )
  }
  const s = params.toString()
  return s ? `?${s}` : ''
}

/** `/v1/repos/{owner}/{name}` + suffix. Owner and name are encoded separately; free-form names go in the query. */
export function repoPath(repo: string, suffix = ''): string {
  const [owner = '', name = ''] = repo.split('/')
  return `/v1/repos/${encodeURIComponent(owner)}/${encodeURIComponent(name)}${suffix}`
}

export function filePath(repo: string, path: string): string {
  return repoPath(repo, '/files/' + path.split('/').map(encodeURIComponent).join('/'))
}

export interface BytesResult {
  bytes: Uint8Array
  /** More than `limit` bytes: nothing was kept. */
  truncated: boolean
  /** Size from `Content-Length` (or the bytes read), when known. */
  size: number | null
}

export class ApiClient {
  private readonly opts: ClientOptions &
    Required<Pick<ClientOptions, 'baseUrl' | 'maxRetries' | 'maxRetryWaitMs' | 'defaultTimeoutMs'>>

  constructor(options: ClientOptions) {
    this.opts = {
      baseUrl: '',
      maxRetries: 3,
      maxRetryWaitMs: 30_000,
      defaultTimeoutMs: 30_000,
      ...options,
    }
  }

  // ---- convenience verbs ------------------------------------------------------------------------

  get<T>(path: string, query?: Query, o: RequestOptions = {}): Promise<T> {
    return this.json<T>(path, { ...o, method: 'GET', query })
  }
  post<T>(path: string, body: unknown = {}, o: RequestOptions = {}): Promise<T> {
    return this.json<T>(path, { ...o, method: 'POST', body })
  }
  put<T>(path: string, body: unknown, o: RequestOptions = {}): Promise<T> {
    return this.json<T>(path, { ...o, method: 'PUT', body })
  }
  patch<T>(path: string, body: unknown, o: RequestOptions = {}): Promise<T> {
    return this.json<T>(path, { ...o, method: 'PATCH', body })
  }
  del<T>(path: string, query?: Query, o: RequestOptions = {}): Promise<T> {
    return this.json<T>(path, { ...o, method: 'DELETE', query })
  }

  // ---- bodies -------------------------------------------------------------------------------------

  json<T>(path: string, o: RequestOptions = {}): Promise<T> {
    return this.run(path, o, async (res) => {
      const text = await res.text()
      if (!text) return undefined as T
      try {
        return JSON.parse(text) as T
      } catch {
        throw new ApiError({
          status: res.status,
          code: 'bad_response',
          message: 'The server sent a response that is not valid JSON',
          requestId: res.headers.get('x-request-id') ?? '',
        })
      }
    })
  }

  /** Reads a binary body, giving up (and keeping nothing) once it exceeds `limit` bytes. */
  bytes(path: string, o: RequestOptions & { limit?: number } = {}): Promise<BytesResult> {
    const limit = o.limit ?? Number.POSITIVE_INFINITY
    return this.run(path, o, async (res) => {
      const declared = res.headers.get('content-length')
      const size = declared !== null && Number.isFinite(Number(declared)) ? Number(declared) : null
      if (size !== null && size > limit) {
        await res.body?.cancel().catch(() => undefined)
        return { bytes: new Uint8Array(0), truncated: true, size }
      }
      if (!res.body) {
        const buf = new Uint8Array(await res.arrayBuffer())
        return buf.length > limit
          ? { bytes: new Uint8Array(0), truncated: true, size: buf.length }
          : { bytes: buf, truncated: false, size: buf.length }
      }
      const reader = res.body.getReader()
      const chunks: Uint8Array[] = []
      let total = 0
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        total += value.length
        if (total > limit) {
          await reader.cancel().catch(() => undefined)
          return { bytes: new Uint8Array(0), truncated: true, size }
        }
        chunks.push(value)
      }
      const out = new Uint8Array(total)
      let at = 0
      for (const c of chunks) {
        out.set(c, at)
        at += c.length
      }
      return { bytes: out, truncated: false, size: size ?? total }
    })
  }

  /**
   * Opens a long-lived response (Server-Sent Events). Retries cover only establishing the connection;
   * the caller owns reading the body and aborting via `signal`.
   */
  open(path: string, o: RequestOptions = {}): Promise<Response> {
    return this.run(path, { ...o, stream: true }, async (res) => res)
  }

  // ---- machinery ----------------------------------------------------------------------------------

  private async run<T>(
    path: string,
    o: RequestOptions,
    consume: (res: Response) => Promise<T>,
  ): Promise<T> {
    const method = o.method ?? 'GET'
    const mutating = MUTATING.has(method) && !o.readOnly
    const key =
      o.idempotencyKey === false
        ? undefined
        : (o.idempotencyKey ??
          (mutating ? (this.opts.newIdempotencyKey?.() ?? `ui-${randomHex(16)}`) : undefined))
    const maxRetries = o.retries ?? this.opts.maxRetries
    const sleep = this.opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
    const random = this.opts.random ?? Math.random
    const baseId = this.opts.newRequestId?.() ?? `ui-${randomHex(8)}`

    for (let attempt = 0; ; attempt++) {
      try {
        const id = attempt === 0 ? baseId : `${baseId}-r${attempt}`
        return await this.attempt(path, o, method, key, id, consume)
      } catch (e) {
        if (!(e instanceof ApiError)) throw e
        if (e.isUnauthenticated && !o.quietUnauthenticated) this.opts.onUnauthenticated?.(e)
        if (
          attempt >= maxRetries ||
          !this.shouldRetry(e, method, key !== undefined || o.readOnly === true)
        )
          throw e
        const backoff = Math.min(8000, 300 * 2 ** attempt) * (0.5 + random() * 0.5)
        const wait = Math.max(e.retryAfterMs, backoff)
        if (wait > this.opts.maxRetryWaitMs) throw e
        await sleep(wait)
        if (o.signal?.aborted) throw cancelled()
      }
    }
  }

  private shouldRetry(e: ApiError, method: HttpMethod, keyed: boolean): boolean {
    if (e.isCancelled) return false
    const safe = method === 'GET' || keyed
    if (e.code === 'network_error' || e.code === 'client_timeout') return safe
    if (!e.retryable) return false
    // 429 and 503 mean the request was refused before it did anything; anything else may have been applied.
    if (e.status === 429 || e.status === 503) return true
    return safe
  }

  private async attempt<T>(
    path: string,
    o: RequestOptions,
    method: HttpMethod,
    key: string | undefined,
    requestId: string,
    consume: (res: Response) => Promise<T>,
  ): Promise<T> {
    if (o.signal?.aborted) throw cancelled()
    const doFetch = this.opts.fetch ?? globalThis.fetch.bind(globalThis)
    const token = o.token === undefined ? this.opts.getToken() : o.token
    const headers: Record<string, string> = {
      accept: o.stream ? 'text/event-stream' : 'application/json',
      'x-request-id': requestId,
      ...o.headers,
    }
    if (token) headers.authorization = `Bearer ${token}`
    if (key) headers['idempotency-key'] = key
    if (o.runId) headers['ag-run'] = o.runId
    const timeoutMs = o.timeoutMs ?? this.opts.defaultTimeoutMs
    if (timeoutMs > 0 && !o.stream) headers['request-timeout'] = String(timeoutMs)
    let body: BodyInit | undefined
    if (o.binary !== undefined && method !== 'GET') {
      headers['content-type'] = 'application/octet-stream'
      body = o.binary
    } else if (o.body !== undefined && method !== 'GET') {
      headers['content-type'] = 'application/json'
      body = JSON.stringify(o.body)
    }

    const controller = new AbortController()
    let timedOut = false
    const onAbort = () => controller.abort()
    o.signal?.addEventListener('abort', onAbort, { once: true })
    const timer =
      timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true
            controller.abort()
          }, timeoutMs + ABORT_GRACE_MS)
        : undefined
    const clearTimer = () => {
      if (timer !== undefined) clearTimeout(timer)
    }
    const unlink = () => o.signal?.removeEventListener('abort', onAbort)
    const failure = (e: unknown): ApiError => {
      if (e instanceof ApiError) return e
      if (o.signal?.aborted) return cancelled()
      if (timedOut) {
        return new ApiError({
          status: 0,
          code: 'client_timeout',
          message: 'The server did not answer in time',
          retryable: true,
          requestId,
        })
      }
      return new ApiError({
        status: 0,
        code: 'network_error',
        message: 'Cannot reach the server',
        retryable: true,
        requestId,
      })
    }

    let res: Response
    try {
      res = await doFetch(this.opts.baseUrl + path + buildQuery(o.query), {
        method,
        headers,
        body,
        signal: controller.signal,
        credentials: 'omit',
        cache: 'no-store',
        referrerPolicy: 'no-referrer',
      })
    } catch (e) {
      clearTimer()
      unlink()
      throw failure(e)
    }

    const date = res.headers.get('date')
    if (date) observeServerDate(Date.parse(date), (this.opts.now ?? Date.now)())

    try {
      if (!res.ok) {
        let parsed: unknown = null
        try {
          const text = await res.text()
          parsed = text ? JSON.parse(text) : null
        } catch {
          parsed = null
        }
        throw errorFromResponse(res.status, parsed, res.headers, (this.opts.now ?? Date.now)())
      }
      const out = await consume(res)
      if (o.stream) {
        // The caller keeps reading; only the connection deadline is over. The abort link stays alive.
        clearTimer()
        return out
      }
      clearTimer()
      unlink()
      return out
    } catch (e) {
      clearTimer()
      unlink()
      throw failure(e)
    }
  }
}

function cancelled(): ApiError {
  return new ApiError({ status: 0, code: 'cancelled', message: 'The request was cancelled' })
}
