// Server-Sent Events over `fetch`. `EventSource` cannot send an Authorization header, and putting a token
// in a URL is not acceptable, so we read the stream ourselves, resume with Last-Event-ID / after_seq,
// ignore heartbeat comments and reconnect with backoff when the connection drops or goes quiet.
import { ApiError, isApiError } from './errors'
import type { ApiClient } from './client'
import { repoPath } from './client'
import type { ServerEvent } from './types'

export interface SseMessage {
  id: string
  event: string
  data: string
  retry?: number
}

/** Incremental parser for the `text/event-stream` format (WHATWG HTML §9.2.6). */
export class SseParser {
  private buffer = ''
  private data: string[] = []
  private event = ''
  private id = ''
  private retry: number | undefined
  private started = false
  private pendingCr = false
  comments = 0

  /** Feed a decoded chunk; returns every message completed by it. */
  push(chunk: string): SseMessage[] {
    let text = chunk
    if (!this.started && text.length > 0) {
      this.started = true
      if (text.charCodeAt(0) === 0xfeff) text = text.slice(1)
    }
    if (this.pendingCr) {
      // A CR at the end of the previous chunk was a complete line ending; swallow a following LF.
      this.pendingCr = false
      if (text.startsWith('\n')) text = text.slice(1)
    }
    this.buffer += text
    const out: SseMessage[] = []
    let start = 0
    for (let i = 0; i < this.buffer.length; i++) {
      const ch = this.buffer[i]
      if (ch !== '\n' && ch !== '\r') continue
      const line = this.buffer.slice(start, i)
      if (ch === '\r') {
        if (this.buffer[i + 1] === '\n') i++
        else if (i + 1 === this.buffer.length) this.pendingCr = true
      }
      start = i + 1
      const msg = this.line(line)
      if (msg) out.push(msg)
    }
    this.buffer = this.buffer.slice(start)
    return out
  }

  private line(line: string): SseMessage | null {
    if (line === '') {
      if (this.data.length === 0) {
        this.event = ''
        return null
      }
      const msg: SseMessage = {
        id: this.id,
        event: this.event || 'message',
        data: this.data.join('\n'),
      }
      if (this.retry !== undefined) msg.retry = this.retry
      this.data = []
      this.event = ''
      return msg
    }
    if (line.startsWith(':')) {
      this.comments++
      return null
    }
    const colon = line.indexOf(':')
    const field = colon === -1 ? line : line.slice(0, colon)
    let value = colon === -1 ? '' : line.slice(colon + 1)
    if (value.startsWith(' ')) value = value.slice(1)
    switch (field) {
      case 'data':
        this.data.push(value)
        break
      case 'event':
        this.event = value
        break
      case 'id':
        if (!value.includes('\0')) this.id = value
        break
      case 'retry':
        if (/^\d+$/.test(value)) this.retry = Number(value)
        break
      default:
        break
    }
    return null
  }
}

export type FeedStatus =
  'idle' | 'connecting' | 'live' | 'reconnecting' | 'closed' | 'unauthenticated' | 'denied'

export interface FeedStatusInfo {
  attempt: number
  error?: ApiError
  /** Milliseconds until the next reconnection attempt, while `reconnecting`. */
  retryInMs?: number
}

export interface FeedOptions {
  client: ApiClient
  repo: string
  topics?: readonly string[]
  /** Deliver events with seq > afterSeq first; omit to start "now". */
  afterSeq?: number
  onEvent: (ev: ServerEvent) => void
  onStatus?: (status: FeedStatus, info: FeedStatusInfo) => void
  /** Treat a connection that produced no bytes (not even heartbeats) for this long as dead. */
  stallMs?: number
  initialBackoffMs?: number
  maxBackoffMs?: number
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>
  random?: () => number
  now?: () => number
}

export interface EventFeed {
  close(): void
  readonly lastSeq: number | null
}

function defaultSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(done, ms)
    function done() {
      clearTimeout(t)
      signal.removeEventListener('abort', done)
      resolve()
    }
    signal.addEventListener('abort', done, { once: true })
  })
}

function parseEvent(data: string): ServerEvent | null {
  try {
    const v = JSON.parse(data) as Partial<ServerEvent> | null
    if (!v || typeof v !== 'object') return null
    if (typeof v.seq !== 'number' || typeof v.topic !== 'string') return null
    return {
      seq: v.seq,
      topic: v.topic,
      repo: typeof v.repo === 'string' ? v.repo : '',
      timestamp_ms: typeof v.timestamp_ms === 'number' ? v.timestamp_ms : 0,
      payload: v.payload && typeof v.payload === 'object' ? v.payload : {},
    }
  } catch {
    return null
  }
}

export function openEventFeed(options: FeedOptions): EventFeed {
  const {
    client,
    repo,
    onEvent,
    onStatus,
    stallMs = 60_000,
    initialBackoffMs = 1000,
    maxBackoffMs = 30_000,
  } = options
  const sleep = options.sleep ?? defaultSleep
  const random = options.random ?? Math.random
  const now = options.now ?? Date.now
  const lifetime = new AbortController()
  let lastSeq: number | null = options.afterSeq ?? null
  let closed = false

  const setStatus = (s: FeedStatus, info: FeedStatusInfo) => onStatus?.(s, info)

  async function readOnce(attempt: number, onConnected: () => void): Promise<'ended'> {
    const connection = new AbortController()
    const stop = () => connection.abort()
    lifetime.signal.addEventListener('abort', stop, { once: true })
    let lastActivity = now()
    const watchdog = setInterval(
      () => {
        if (now() - lastActivity > stallMs) connection.abort()
      },
      Math.max(250, Math.floor(stallMs / 4)),
    )
    try {
      setStatus(attempt === 0 ? 'connecting' : 'reconnecting', { attempt })
      const res = await client.open(repoPath(repo, '/events'), {
        query: {
          after_seq: lastSeq === null ? undefined : lastSeq,
          topics: options.topics && options.topics.length > 0 ? [...options.topics] : undefined,
        },
        headers: lastSeq === null ? undefined : { 'last-event-id': String(lastSeq) },
        signal: connection.signal,
        retries: 0,
        timeoutMs: 20_000,
      })
      lastActivity = now()
      onConnected()
      setStatus('live', { attempt })
      if (!res.body) return 'ended'
      const reader = res.body.getReader()
      const decoder = new TextDecoder('utf-8')
      const parser = new SseParser()
      for (;;) {
        const { done, value } = await reader.read()
        if (done) return 'ended'
        lastActivity = now()
        for (const msg of parser.push(decoder.decode(value, { stream: true }))) {
          if (msg.event === 'error') throw parseStreamError(msg.data)
          const ev = parseEvent(msg.data)
          if (!ev) continue
          if (lastSeq !== null && ev.seq <= lastSeq) continue // replay overlap: already delivered
          lastSeq = ev.seq
          onEvent(ev)
        }
      }
    } finally {
      clearInterval(watchdog)
      lifetime.signal.removeEventListener('abort', stop)
      connection.abort()
    }
  }

  async function loop(): Promise<void> {
    let attempt = 0
    while (!closed) {
      let error: ApiError | undefined
      let connected = false
      try {
        await readOnce(attempt, () => (connected = true))
      } catch (e) {
        if (closed) break
        if (isApiError(e)) {
          error = e
          if (e.isUnauthenticated) {
            setStatus('unauthenticated', { attempt, error: e })
            return
          }
          if (e.isForbidden || e.isNotFound) {
            setStatus('denied', { attempt, error: e })
            return
          }
        } else {
          // The stream broke mid-read (network drop, stall watchdog): treat like a lost connection.
          error = new ApiError({
            status: 0,
            code: 'network_error',
            message: 'The event stream was interrupted',
            retryable: true,
          })
        }
      }
      if (closed) break
      // A connection that was established counts as healthy: the next backoff starts over.
      attempt = connected ? 1 : attempt + 1
      const base = Math.min(maxBackoffMs, initialBackoffMs * 2 ** Math.min(attempt - 1, 10))
      const delay = Math.max(error?.retryAfterMs ?? 0, base * (0.5 + random() * 0.5))
      setStatus('reconnecting', { attempt, error, retryInMs: delay })
      await sleep(delay, lifetime.signal)
    }
  }

  void loop().finally(() => {
    if (closed) setStatus('closed', { attempt: 0 })
  })

  return {
    close() {
      if (closed) return
      closed = true
      lifetime.abort()
    },
    get lastSeq() {
      return lastSeq
    },
  }
}

function parseStreamError(data: string): ApiError {
  try {
    const body = JSON.parse(data) as { error?: Record<string, unknown> }
    const e = body.error ?? {}
    return new ApiError({
      status: 0,
      code: typeof e.code === 'string' ? e.code : 'internal',
      message: typeof e.message === 'string' ? e.message : 'The event stream reported an error',
      retryable: e.retryable === true,
      retryAfterMs: typeof e.retry_after_ms === 'number' ? e.retry_after_ms : 0,
      requestId: typeof e.request_id === 'string' ? e.request_id : '',
    })
  } catch {
    return new ApiError({
      status: 0,
      code: 'internal',
      message: 'The event stream reported an error',
    })
  }
}
