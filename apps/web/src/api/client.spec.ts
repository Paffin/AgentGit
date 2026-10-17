import { describe, expect, it, vi } from 'vitest'
import { ApiClient, buildQuery, filePath, repoPath } from './client'
import type { ClientOptions } from './client'
import { ApiError } from './errors'

type Call = { url: string; init: RequestInit; headers: Headers }

function json(body: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...headers },
  })
}
function errorBody(
  code: string,
  extra: Record<string, unknown> = {},
  status = 400,
  headers: Record<string, string> = {},
) {
  return json(
    {
      error: {
        code,
        message: code,
        retryable: false,
        retry_after_ms: 0,
        request_id: 'req_x',
        details: {},
        ...extra,
      },
    },
    status,
    headers,
  )
}

function harness(
  responses: Array<Response | Error | (() => Response | Promise<Response>)>,
  opts: Partial<ClientOptions> = {},
) {
  const calls: Call[] = []
  const queue = [...responses]
  const fetchMock = vi.fn(async (url: RequestInfo | URL, init: RequestInit = {}) => {
    calls.push({ url: String(url), init, headers: new Headers(init.headers) })
    const next = queue.shift()
    if (next === undefined) throw new Error('no more responses queued')
    if (next instanceof Error) throw next
    return typeof next === 'function' ? next() : next
  })
  const sleeps: number[] = []
  const client = new ApiClient({
    getToken: () => 'agt_secret_token_value',
    fetch: fetchMock as unknown as typeof fetch,
    sleep: async (ms) => void sleeps.push(ms),
    random: () => 0.5,
    newRequestId: () => 'ui-test',
    newIdempotencyKey: () => 'key-1',
    ...opts,
  })
  return { client, calls, sleeps, fetchMock }
}

describe('request shape', () => {
  it('sends the bearer token, a request id and a deadline, and never puts the token in the URL', async () => {
    const { client, calls } = harness([json({ ok: true })])
    await client.get('/v1/whoami')
    const c = calls[0]!
    expect(c.headers.get('authorization')).toBe('Bearer agt_secret_token_value')
    expect(c.headers.get('x-request-id')).toBe('ui-test')
    expect(c.headers.get('request-timeout')).toBe('30000')
    expect(c.url).toBe('/v1/whoami')
    expect(c.url).not.toContain('agt_')
    expect(c.init.credentials).toBe('omit')
    expect(c.init.referrerPolicy).toBe('no-referrer')
  })

  it('sends no Authorization header without a token', async () => {
    const { client, calls } = harness([json({})], { getToken: () => null })
    await client.get('/v1/capabilities')
    expect(calls[0]!.headers.has('authorization')).toBe(false)
  })

  it('lets one call override the token (checking a pasted token)', async () => {
    const { client, calls } = harness([json({})])
    await client.get('/v1/whoami', undefined, { token: 'agt_other_token_value' })
    expect(calls[0]!.headers.get('authorization')).toBe('Bearer agt_other_token_value')
  })

  it('adds an idempotency key to mutations only', async () => {
    const { client, calls } = harness([json({}), json({}), json({}), json({})])
    await client.get('/v1/repos')
    await client.post('/v1/repos', { name: 'a/b' })
    await client.put('/v1/x', {})
    await client.del('/v1/y')
    expect(calls[0]!.headers.has('idempotency-key')).toBe(false)
    expect(calls[1]!.headers.get('idempotency-key')).toBe('key-1')
    expect(calls[2]!.headers.get('idempotency-key')).toBe('key-1')
    expect(calls[3]!.headers.get('idempotency-key')).toBe('key-1')
    expect(calls[1]!.init.body).toBe(JSON.stringify({ name: 'a/b' }))
    expect(calls[1]!.headers.get('content-type')).toBe('application/json')
  })

  it('honours an explicit key, and read-only POSTs get none', async () => {
    const { client, calls } = harness([json({}), json({})])
    await client.post('/v1/a', {}, { idempotencyKey: 'mine' })
    await client.post('/v1/diff', {}, { readOnly: true })
    expect(calls[0]!.headers.get('idempotency-key')).toBe('mine')
    expect(calls[1]!.headers.has('idempotency-key')).toBe(false)
  })

  it('acts as an agent run when asked, and sets the deadline it was given', async () => {
    const { client, calls } = harness([json({})])
    await client.post('/v1/x', {}, { runId: 'run_1', timeoutMs: 5000 })
    expect(calls[0]!.headers.get('ag-run')).toBe('run_1')
    expect(calls[0]!.headers.get('request-timeout')).toBe('5000')
  })

  it('builds queries without empty values and joins lists', () => {
    expect(buildQuery({ a: 'x', b: undefined, c: '', d: 3, e: false, f: ['p', 'q'] })).toBe(
      '?a=x&d=3&e=false&f=p%2Cq',
    )
    expect(buildQuery({})).toBe('')
  })

  it('encodes repository and file paths segment by segment', () => {
    expect(repoPath('acme/web', '/log')).toBe('/v1/repos/acme/web/log')
    expect(filePath('acme/web', 'src/a b/ü.ts')).toBe(
      '/v1/repos/acme/web/files/src/a%20b/%C3%BC.ts',
    )
  })
})

describe('errors and retries', () => {
  it('raises a typed error carrying code, retryability and request id', async () => {
    const { client } = harness([errorBody('permission_denied', { message: 'no' }, 403)])
    const err = await client.get('/v1/x').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).code).toBe('permission_denied')
    expect((err as ApiError).requestId).toBe('req_x')
    expect((err as ApiError).retryable).toBe(false)
  })

  it('does not retry errors that are not retryable', async () => {
    const { client, calls } = harness([errorBody('invalid_argument')])
    await expect(client.get('/v1/x')).rejects.toBeInstanceOf(ApiError)
    expect(calls).toHaveLength(1)
  })

  it('retries retryable errors and honours Retry-After', async () => {
    const { client, calls, sleeps } = harness([
      errorBody('rate_limited', { retryable: true, retry_after_ms: 2000 }, 429, {
        'retry-after': '2',
      }),
      json({ done: true }),
    ])
    const out = await client.get<{ done: boolean }>('/v1/x')
    expect(out.done).toBe(true)
    expect(calls).toHaveLength(2)
    expect(sleeps).toEqual([2000])
  })

  it('gives up after the retry budget', async () => {
    const busy = () => errorBody('unavailable', { retryable: true }, 503)
    const { client, calls } = harness([busy(), busy(), busy(), busy(), busy()], { maxRetries: 2 })
    await expect(client.get('/v1/x')).rejects.toMatchObject({ code: 'unavailable' })
    expect(calls).toHaveLength(3)
  })

  it('does not sit through a Retry-After longer than it is willing to wait', async () => {
    const { client, calls } = harness([
      errorBody('rate_limited', { retryable: true, retry_after_ms: 120_000 }, 429),
    ])
    await expect(client.get('/v1/x')).rejects.toMatchObject({
      code: 'rate_limited',
      retryAfterMs: 120_000,
    })
    expect(calls).toHaveLength(1)
  })

  it('reuses the SAME idempotency key when a mutation is retried', async () => {
    const { client, calls } = harness([
      errorBody('deadline_exceeded', { retryable: true }, 504),
      json({ ok: 1 }),
    ])
    await client.post('/v1/repos/a/b/proposals', { title: 't' })
    expect(calls).toHaveLength(2)
    expect(calls[0]!.headers.get('idempotency-key')).toBe('key-1')
    expect(calls[1]!.headers.get('idempotency-key')).toBe('key-1')
    expect(calls[1]!.headers.get('x-request-id')).toBe('ui-test-r1')
  })

  it('retries a network failure for GETs and keyed mutations, but not for keyless ones', async () => {
    const down = () => new TypeError('Failed to fetch')
    const get = harness([down(), json({ n: 1 })])
    await expect(get.client.get('/v1/x')).resolves.toEqual({ n: 1 })

    const keyed = harness([down(), json({ n: 2 })])
    await expect(keyed.client.post('/v1/x', {})).resolves.toEqual({ n: 2 })

    const keyless = harness([down(), json({ n: 3 })])
    await expect(keyless.client.post('/v1/x', {}, { idempotencyKey: false })).rejects.toMatchObject(
      { code: 'network_error' },
    )
    expect(keyless.calls).toHaveLength(1)
  })

  it('retries a keyless mutation only when the server proves it did nothing (429/503)', async () => {
    const { client, calls } = harness([
      errorBody('rate_limited', { retryable: true }, 429),
      json({ ok: true }),
    ])
    await client.post('/v1/x', {}, { idempotencyKey: false })
    expect(calls).toHaveLength(2)
    const timeout = harness([errorBody('deadline_exceeded', { retryable: true }, 504)])
    await expect(timeout.client.post('/v1/x', {}, { idempotencyKey: false })).rejects.toMatchObject(
      { code: 'deadline_exceeded' },
    )
    expect(timeout.calls).toHaveLength(1)
  })

  it('reports 401 to the session unless told to stay quiet', async () => {
    const onUnauthenticated = vi.fn()
    const loud = harness([errorBody('unauthenticated', {}, 401)], { onUnauthenticated })
    await expect(loud.client.get('/v1/x')).rejects.toMatchObject({ code: 'unauthenticated' })
    expect(onUnauthenticated).toHaveBeenCalledTimes(1)

    const quiet = harness([errorBody('unauthenticated', {}, 401)], { onUnauthenticated })
    await expect(
      quiet.client.get('/v1/whoami', undefined, { quietUnauthenticated: true }),
    ).rejects.toBeInstanceOf(ApiError)
    expect(onUnauthenticated).toHaveBeenCalledTimes(1)
  })

  it('survives a non-JSON success body', async () => {
    const { client } = harness([new Response('<html>', { status: 200 })])
    await expect(client.get('/v1/x')).rejects.toMatchObject({ code: 'bad_response' })
  })

  it('maps a client-side abort to a cancelled error and never retries it', async () => {
    const ctl = new AbortController()
    const fetchMock = vi.fn(
      (_: unknown, init?: RequestInit) =>
        new Promise<Response>((_res, reject) =>
          init?.signal?.addEventListener('abort', () =>
            reject(new DOMException('aborted', 'AbortError')),
          ),
        ),
    )
    const client = new ApiClient({
      getToken: () => 't',
      fetch: fetchMock as unknown as typeof fetch,
      sleep: async () => undefined,
    })
    const p = client.get('/v1/x', undefined, { signal: ctl.signal })
    await Promise.resolve()
    ctl.abort()
    await expect(p).rejects.toMatchObject({ code: 'cancelled' })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('times out slow servers with a retryable client_timeout', async () => {
    vi.useFakeTimers()
    const hang = (init?: RequestInit) =>
      new Promise<Response>((_, reject) =>
        init?.signal?.addEventListener('abort', () =>
          reject(new DOMException('aborted', 'AbortError')),
        ),
      )
    const fetchMock = vi.fn((_: unknown, init?: RequestInit) => hang(init))
    const client = new ApiClient({
      getToken: () => 't',
      fetch: fetchMock as unknown as typeof fetch,
      maxRetries: 0,
      sleep: async () => undefined,
    })
    const p = client.get('/v1/x', undefined, { timeoutMs: 1000 })
    const assertion = expect(p).rejects.toMatchObject({ code: 'client_timeout', retryable: true })
    await vi.advanceTimersByTimeAsync(5000)
    await assertion
  })
})

describe('bytes', () => {
  const body = (n: number) =>
    new Response(new Uint8Array(n).fill(97), {
      status: 200,
      headers: { 'content-length': String(n) },
    })

  it('returns the bytes of a small file', async () => {
    const { client } = harness([body(5)])
    const r = await client.bytes('/v1/f', { limit: 100 })
    expect(r.truncated).toBe(false)
    expect(r.bytes.length).toBe(5)
    expect(r.size).toBe(5)
  })

  it('refuses a file whose declared size is over the limit, without reading it', async () => {
    const { client } = harness([body(500)])
    const r = await client.bytes('/v1/f', { limit: 100 })
    expect(r.truncated).toBe(true)
    expect(r.bytes.length).toBe(0)
    expect(r.size).toBe(500)
  })

  it('cuts off an undeclared size once it passes the limit', async () => {
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(new Uint8Array(60))
        c.enqueue(new Uint8Array(60))
        c.close()
      },
    })
    const { client } = harness([new Response(stream, { status: 200 })])
    const r = await client.bytes('/v1/f', { limit: 100 })
    expect(r.truncated).toBe(true)
  })
})
