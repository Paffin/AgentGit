import { describe, expect, it, vi } from 'vitest'
import { SseParser, openEventFeed } from './sse'
import type { FeedStatus } from './sse'
import type { ApiClient } from './client'
import { ApiError } from './errors'
import type { ServerEvent } from './types'

describe('SseParser', () => {
  it('parses events with id, event and data', () => {
    const p = new SseParser()
    const out = p.push('id: 7\nevent: proposal.create\ndata: {"a":1}\n\n')
    expect(out).toEqual([{ id: '7', event: 'proposal.create', data: '{"a":1}' }])
  })

  it('handles events split across arbitrary chunk boundaries', () => {
    const p = new SseParser()
    const wire = 'id: 1\nevent: x\ndata: hel\ndata: lo\n\nid: 2\ndata: second\n\n'
    const got = []
    for (const ch of wire) got.push(...p.push(ch))
    expect(got).toEqual([
      { id: '1', event: 'x', data: 'hel\nlo' },
      { id: '2', event: 'message', data: 'second' },
    ])
  })

  it('accepts CRLF and bare CR line endings, even when CR and LF are split', () => {
    const p = new SseParser()
    expect(p.push('data: a\r\n\r\n')).toHaveLength(1)
    expect(p.push('data: b\r\r')).toHaveLength(1)
    const split = [...p.push('data: c\r'), ...p.push('\n\r'), ...p.push('\n')]
    expect(split).toEqual([{ id: '', event: 'message', data: 'c' }])
  })

  it('ignores comments (heartbeats) but counts them', () => {
    const p = new SseParser()
    expect(p.push(': heartbeat\n\n')).toEqual([])
    expect(p.push(':\n')).toEqual([])
    expect(p.comments).toBe(2)
  })

  it('does not dispatch an event with no data', () => {
    const p = new SseParser()
    expect(p.push('event: ping\n\n')).toEqual([])
    expect(p.push('data: x\n\n')[0]?.event).toBe('message') // the stray event name did not leak
  })

  it('strips a byte order mark and one leading space', () => {
    const p = new SseParser()
    expect(p.push('﻿data:  two spaces\n\n')[0]?.data).toBe(' two spaces')
  })

  it('reads retry and ignores ids with NUL', () => {
    const p = new SseParser()
    const out = p.push('retry: 3000\nid: a\0b\ndata: x\n\n')
    expect(out[0]?.retry).toBe(3000)
    expect(out[0]?.id).toBe('')
  })

  it('ignores unknown fields', () => {
    expect(new SseParser().push('foo: bar\ndata: x\n\n')).toHaveLength(1)
  })
})

// ---- the feed -------------------------------------------------------------------------------------

const enc = new TextEncoder()
const ev = (seq: number, topic = 'proposal.create'): ServerEvent => ({
  seq,
  topic,
  repo: 'a/b',
  timestamp_ms: 1,
  payload: {},
})
const frame = (e: ServerEvent) => `id: ${e.seq}\nevent: ${e.topic}\ndata: ${JSON.stringify(e)}\n\n`

class Conn {
  controller!: ReadableStreamDefaultController<Uint8Array>
  readonly response: Response
  constructor() {
    const stream = new ReadableStream<Uint8Array>({
      start: (c) => {
        this.controller = c
      },
    })
    this.response = new Response(stream, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    })
  }
  send(text: string) {
    this.controller.enqueue(enc.encode(text))
  }
  end() {
    this.controller.close()
  }
}

function setup(opts: { stallMs?: number } = {}) {
  const queue: Array<Conn | ApiError> = []
  const opens: Array<{
    query: Record<string, unknown> | undefined
    headers: Record<string, string> | undefined
  }> = []
  const client = {
    open: vi.fn(
      async (
        _path: string,
        o: {
          query?: Record<string, unknown>
          headers?: Record<string, string>
          signal?: AbortSignal
        },
      ) => {
        opens.push({ query: o.query, headers: o.headers })
        const next = queue.shift()
        if (!next)
          throw new ApiError({ status: 0, code: 'network_error', message: 'down', retryable: true })
        if (next instanceof ApiError) throw next
        o.signal?.addEventListener('abort', () => {
          try {
            next.controller.error(new DOMException('aborted', 'AbortError'))
          } catch {
            /* already closed */
          }
        })
        return next.response
      },
    ),
  } as unknown as ApiClient
  const events: ServerEvent[] = []
  const statuses: FeedStatus[] = []
  const delays: number[] = []
  let release: (() => void) | null = null
  const feedOptions = {
    client,
    repo: 'a/b',
    onEvent: (e: ServerEvent) => events.push(e),
    onStatus: (s: FeedStatus) => statuses.push(s),
    sleep: (ms: number) => {
      delays.push(ms)
      return new Promise<void>((r) => {
        release = r
      })
    },
    random: () => 0.5,
    initialBackoffMs: 1000,
    maxBackoffMs: 8000,
    stallMs: opts.stallMs,
  }
  return {
    queue,
    opens,
    events,
    statuses,
    delays,
    feedOptions,
    proceed: () => release?.(),
  }
}

const tick = async (n = 6) => {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

describe('openEventFeed', () => {
  it('delivers events in order and reports live', async () => {
    const h = setup()
    const conn = new Conn()
    h.queue.push(conn)
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 10 })
    await tick()
    conn.send(frame(ev(11)) + ': heartbeat\n\n' + frame(ev(12)))
    await tick()
    expect(h.events.map((e) => e.seq)).toEqual([11, 12])
    expect(h.statuses).toContain('live')
    expect(feed.lastSeq).toBe(12)
    expect(h.opens[0]!.query).toMatchObject({ after_seq: 10 })
    feed.close()
    await tick()
    expect(h.statuses.at(-1)).toBe('closed')
  })

  it('starts "from now" when no resume point is known', async () => {
    const h = setup()
    h.queue.push(new Conn())
    const feed = openEventFeed(h.feedOptions)
    await tick()
    expect(h.opens[0]!.query!.after_seq).toBeUndefined()
    expect(h.opens[0]!.headers).toBeUndefined()
    feed.close()
  })

  it('reconnects with backoff and resumes after the last event it saw (Last-Event-ID + after_seq)', async () => {
    const h = setup()
    const first = new Conn()
    const second = new Conn()
    h.queue.push(first, second)
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    first.send(frame(ev(1)) + frame(ev(2)))
    await tick()
    first.end() // the server went away
    await tick()
    expect(h.statuses.at(-1)).toBe('reconnecting')
    expect(h.delays).toEqual([750]) // 1000 * (0.5 + 0.5 * 0.5): jittered, after a healthy connection
    h.proceed()
    await tick()
    expect(h.opens[1]!.query).toMatchObject({ after_seq: 2 })
    expect(h.opens[1]!.headers).toEqual({ 'last-event-id': '2' })
    second.send(frame(ev(2)) + frame(ev(3))) // an overlap on replay is delivered once
    await tick()
    expect(h.events.map((e) => e.seq)).toEqual([1, 2, 3])
    expect(h.statuses.at(-1)).toBe('live')
    feed.close()
  })

  it('backs off exponentially while the server stays unreachable', async () => {
    const h = setup()
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    for (let i = 0; i < 4; i++) {
      await tick()
      h.proceed()
    }
    await tick()
    expect(h.delays.slice(0, 4)).toEqual([750, 1500, 3000, 6000])
    feed.close()
  })

  it('caps the backoff', async () => {
    const h = setup()
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    for (let i = 0; i < 8; i++) {
      await tick()
      h.proceed()
    }
    await tick()
    expect(Math.max(...h.delays)).toBeLessThanOrEqual(8000)
    feed.close()
  })

  it('stops on 401 and reports the session as ended', async () => {
    const h = setup()
    h.queue.push(new ApiError({ status: 401, code: 'unauthenticated', message: 'expired' }))
    openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    expect(h.statuses.at(-1)).toBe('unauthenticated')
    expect(h.opens).toHaveLength(1)
  })

  it('stops on 403/404 instead of hammering the server', async () => {
    const h = setup()
    h.queue.push(new ApiError({ status: 404, code: 'not_found', message: 'no such repo' }))
    openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    expect(h.statuses.at(-1)).toBe('denied')
    expect(h.opens).toHaveLength(1)
  })

  it('treats an in-stream error event as the end of the session when it says so', async () => {
    const h = setup()
    const conn = new Conn()
    h.queue.push(conn)
    openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    conn.send(
      'event: error\ndata: {"error":{"code":"unauthenticated","message":"the credential expired"}}\n\n',
    )
    await tick()
    expect(h.statuses.at(-1)).toBe('unauthenticated')
  })

  it('ignores malformed frames without dying', async () => {
    const h = setup()
    const conn = new Conn()
    h.queue.push(conn)
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    conn.send('data: not json\n\ndata: {"seq":"x"}\n\n' + frame(ev(5)))
    await tick()
    expect(h.events.map((e) => e.seq)).toEqual([5])
    feed.close()
  })

  it('treats a silent connection as dead after stallMs and reconnects', async () => {
    const h = setup({ stallMs: 80 })
    const first = new Conn()
    h.queue.push(first, new Conn())
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 4 })
    await tick()
    await new Promise((r) => setTimeout(r, 260)) // no bytes, not even heartbeats
    await tick()
    expect(h.statuses).toContain('reconnecting')
    h.proceed()
    await tick()
    expect(h.opens).toHaveLength(2)
    feed.close()
  })

  it('keeps a connection alive that only sends heartbeats', async () => {
    const h = setup({ stallMs: 120 })
    const conn = new Conn()
    h.queue.push(conn)
    const feed = openEventFeed({ ...h.feedOptions, afterSeq: 0 })
    await tick()
    for (let i = 0; i < 6; i++) {
      conn.send(': heartbeat\n\n')
      await new Promise((r) => setTimeout(r, 50))
    }
    expect(h.opens).toHaveLength(1)
    expect(h.statuses.at(-1)).toBe('live')
    feed.close()
  })
})
