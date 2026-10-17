// Server-Sent Events against a real server: replay, live follow, heartbeats, reconnect with resume.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AgentGitClient, readSse, type AgentEvent } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
const repo = 'acme/events';
let admin: AgentGitClient;
let alice: AgentGitClient;

beforeAll(async () => {
  server = await startServer();
  admin = server.admin();
  await admin.repos.create({ name: repo });
  alice = await server.actor('user:alice', repo, 'writer');
});
afterAll(async () => {
  await server?.stop();
});

/** Makes `n` distinct events (each workspace creation appends one `workspace.create` event). */
async function makeEvents(prefix: string, n: number): Promise<void> {
  for (let i = 0; i < n; i++) await alice.workspaces.create({ repo, name: `${prefix}-${i}`, stream: 'main' });
}

async function take(it: AsyncGenerator<AgentEvent>, n: number, ms = 15_000): Promise<AgentEvent[]> {
  const out: AgentEvent[] = [];
  const deadline = Date.now() + ms;
  while (out.length < n) {
    const next = await Promise.race([
      it.next(),
      new Promise<never>((_, reject) =>
        setTimeout(
          () => reject(new Error(`timed out after ${out.length}/${n} events`)),
          Math.max(1, deadline - Date.now()),
        ),
      ),
    ]);
    if (next.done) break;
    out.push(next.value);
  }
  return out;
}

describe('replay', () => {
  it('afterSeq 0 replays the whole history in order, then the iterator can be stopped by break', async () => {
    await makeEvents('replay', 3);
    const replayed = await alice.events.replay({ repo, after_seq: 0, limit: 1000 });
    const total = replayed.events.length;
    expect(total).toBeGreaterThanOrEqual(4); // repo.init + 3 workspace.create

    const seen: number[] = [];
    for await (const ev of alice.subscribeEvents({ repo, afterSeq: 0 })) {
      seen.push(ev.seq);
      if (seen.length === total) break;
    }
    expect(seen).toEqual(replayed.events.map((e) => e.seq));
  });

  it('resumes after a given seq and filters by topic prefix', async () => {
    const all = (await alice.events.replay({ repo, after_seq: 0, limit: 1000 })).events;
    const first = all[0]!;
    const abort = new AbortController();
    const got: AgentEvent[] = [];
    for await (const ev of alice.subscribeEvents({
      repo,
      afterSeq: first.seq,
      topics: ['workspace'],
      signal: abort.signal,
    })) {
      got.push(ev);
      if (got.length === all.filter((e) => e.topic.startsWith('workspace') && e.seq > first.seq).length) abort.abort();
    }
    expect(got.every((e) => e.topic.startsWith('workspace.') && e.seq > first.seq)).toBe(true);
    expect(got.length).toBeGreaterThanOrEqual(3);
  });
});

describe('live', () => {
  it('without afterSeq only new events arrive, and heartbeats are never yielded', async () => {
    await makeEvents('before-live', 1);
    const abort = new AbortController();
    const it = alice.subscribeEvents({ repo, heartbeatMs: 50, signal: abort.signal });
    const pending = take(it, 2);
    // Several heartbeats pass without producing anything.
    await new Promise((r) => setTimeout(r, 400));
    await makeEvents('live', 2);
    const events = await pending;
    expect(events.map((e) => e.payload['target'])).toEqual(['live-0', 'live-1']);
    expect(events.every((e) => e.topic === 'workspace.create' && e.repo === repo)).toBe(true);
    abort.abort();
    expect((await it.next()).done).toBe(true);
  });

  it('aborting the signal ends the iterator quietly, even while waiting', async () => {
    const abort = new AbortController();
    const it = alice.subscribeEvents({ repo, signal: abort.signal });
    const next = it.next();
    setTimeout(() => abort.abort(), 100);
    expect((await next).done).toBe(true);
  });

  it('a caller without access gets a typed error at the first read', async () => {
    const eve = await server.actor('user:eve', 'other/*', 'reader');
    const it = eve.subscribeEvents({ repo, afterSeq: 0 });
    await expect(it.next()).rejects.toMatchObject({ code: 'not_found' });
  });

  it('sends the same events the replay API returns, with the payload as a JSON object', async () => {
    const one = (await alice.events.replay({ repo, after_seq: 0, limit: 1 })).events[0]!;
    const [viaSse] = await take(alice.subscribeEvents({ repo, afterSeq: 0 }), 1);
    expect(viaSse).toEqual(one);
    expect(one.payload['actor']).toBeDefined();
  });
});

/**
 * Wraps `fetch` so that the n-th event-stream connection breaks after `plan[n].after` events:
 * frames are forwarded one by one, then the stream is closed cleanly or fails like a dropped socket.
 */
function breakingStreams(
  plan: Array<{ after: number; how: 'close' | 'error' }>,
  log: { urls: string[]; cancelled: number },
): typeof fetch {
  let connection = 0;
  return async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const res = await fetch(input, init);
    if (!url.includes('/events?') || !res.body) return res;
    log.urls.push(url);
    const step = plan[connection++];
    if (!step) return res;
    const upstream = res.body.getReader();
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    let buffer = '';
    let events = 0;
    const body = new ReadableStream<Uint8Array>({
      async pull(controller) {
        for (;;) {
          const cut = buffer.indexOf('\n\n');
          if (cut >= 0) {
            const frame = buffer.slice(0, cut + 2);
            buffer = buffer.slice(cut + 2);
            controller.enqueue(encoder.encode(frame));
            if (frame.startsWith('id:')) events++;
            if (events >= step.after) {
              await upstream.cancel();
              if (step.how === 'close') controller.close();
              else controller.error(new TypeError('terminated (injected)'));
            }
            return;
          }
          const { done, value } = await upstream.read();
          if (done) return controller.close();
          buffer += decoder.decode(value, { stream: true });
        }
      },
      cancel: async () => {
        log.cancelled++;
        await upstream.cancel();
      },
    });
    return new Response(body, { status: res.status, headers: res.headers });
  };
}

describe('stopping', () => {
  it('leaving the loop early closes the connection to the server', async () => {
    let cancelled = 0;
    const tracking: typeof fetch = async (input, init) => {
      const res = await fetch(input, init);
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
      if (!url.includes('/events?') || !res.body) return res;
      const upstream = res.body.getReader();
      const body = new ReadableStream<Uint8Array>({
        async pull(controller) {
          const { done, value } = await upstream.read();
          if (done) controller.close();
          else controller.enqueue(value);
        },
        async cancel(reason) {
          cancelled++;
          await upstream.cancel(reason);
        },
      });
      return new Response(body, { status: res.status, headers: res.headers });
    };
    const client = new AgentGitClient({ baseUrl: server.baseUrl, token: server.bootstrapToken, fetch: tracking });
    for await (const _event of client.subscribeEvents({ repo, afterSeq: 0 })) break;
    expect(cancelled).toBe(1);
  });
});

describe('reconnect', () => {
  it.each([['close' as const], ['error' as const]])(
    'resumes without gaps or duplicates when the connection %ss mid-stream',
    async (how) => {
      await makeEvents(`rc-${how}`, 6);
      const expected = (await alice.events.replay({ repo, after_seq: 0, limit: 1000 })).events.map((e) => e.seq);
      const log = { urls: [] as string[], cancelled: 0 };
      const client = new AgentGitClient({
        baseUrl: server.baseUrl,
        token: server.bootstrapToken,
        fetch: breakingStreams(
          [
            { after: 2, how },
            { after: 2, how },
          ],
          log,
        ),
        sleep: async () => undefined,
      });
      const seqs: number[] = [];
      for await (const ev of client.subscribeEvents({ repo, afterSeq: 0 })) {
        seqs.push(ev.seq);
        if (seqs.length === expected.length) break;
      }
      expect(seqs).toEqual(expected);
      // The first connection replays from 0; each reconnect asks for events after the last one it got.
      expect(log.urls.length).toBeGreaterThanOrEqual(3);
      expect(new URL(log.urls[0]!).searchParams.get('after_seq')).toBe('0');
      const resumeFrom = log.urls.slice(1).map((u) => Number(new URL(u).searchParams.get('after_seq')));
      expect(resumeFrom[0]).toBeGreaterThan(0);
      expect(resumeFrom).toEqual([...resumeFrom].sort((a, b) => a - b));
      expect(expected).toContain(resumeFrom[0]);
    },
  );

  it('from "now", a reconnect picks up events appended while it was away', async () => {
    const log = { urls: [] as string[], cancelled: 0 };
    let drop: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      drop = resolve;
    });
    // First connection: let it live until `drop`, then break it; the next one is untouched.
    const real = breakingStreams([{ after: 1, how: 'error' }], log);
    const client = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      fetch: async (input, init) => {
        const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
        if (url.includes('/events?') && log.urls.length === 1) await gate; // delay the reconnect
        return real(input, init);
      },
    });
    const it = client.subscribeEvents({ repo });
    const first = take(it, 1);
    await new Promise((r) => setTimeout(r, 300));
    await makeEvents('away-a', 1); // delivered on the first connection, which then breaks
    expect((await first).map((e) => e.payload['target'])).toEqual(['away-a-0']);
    // While the client is disconnected, more events happen ...
    const next = take(it, 2);
    await makeEvents('away-b', 2);
    drop?.();
    // ... and none of them is lost.
    expect((await next).map((e) => e.payload['target'])).toEqual(['away-b-0', 'away-b-1']);
    await it.return(undefined);
  });

  it('stops retrying on a non-retryable error such as an expired credential', async () => {
    const short = await server.actor('user:shorty', repo, 'reader');
    await admin.credentials.revoke({ credential_id: (await short.session.whoami()).credential_id });
    await expect(short.subscribeEvents({ repo, afterSeq: 0 }).next()).rejects.toMatchObject({
      code: 'unauthenticated',
    });
  });

  it('survives a stream that goes silent: the idle watchdog reconnects', async () => {
    const log = { urls: [] as string[], cancelled: 0 };
    let connections = 0;
    const silentFirst: typeof fetch = async (input, init) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
      if (url.includes('/events?') && connections++ === 0) {
        log.urls.push(url);
        // A connection that opens and then says nothing at all (no heartbeats).
        const body = new ReadableStream<Uint8Array>({ start() {}, cancel: () => void log.cancelled++ });
        return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
      }
      return fetch(input, init);
    };
    const client = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      fetch: silentFirst,
      sleep: async () => undefined,
    });
    const evs = await take(client.subscribeEvents({ repo, afterSeq: 0, idleTimeoutMs: 200 }), 2);
    expect(evs).toHaveLength(2);
    expect(log.cancelled).toBe(1);
    expect(connections).toBeGreaterThanOrEqual(2);
  });
});

describe('the wire format', () => {
  it('honours Last-Event-ID as the protocol documents, and sends comments as heartbeats', async () => {
    const all = (await alice.events.replay({ repo, after_seq: 0, limit: 1000 })).events;
    const resumeAfter = all[all.length - 3]!.seq;
    const res = await fetch(`${server.baseUrl}/v1/repos/${repo}/events?heartbeat_ms=50`, {
      headers: { authorization: `Bearer ${server.bootstrapToken}`, 'last-event-id': String(resumeAfter) },
    });
    expect(res.headers.get('content-type')).toContain('text/event-stream');
    const ids: string[] = [];
    const raw: string[] = [];
    const decoder = new TextDecoder();
    const [forFrames, forRaw] = res.body!.tee();
    const rawReader = forRaw.getReader();
    const collectRaw = (async () => {
      for (;;) {
        const { done, value } = await rawReader.read();
        if (done) return;
        raw.push(decoder.decode(value));
        if (raw.join('').includes(': heartbeat')) return;
      }
    })();
    for await (const frame of readSse(forFrames)) {
      ids.push(frame.id);
      if (ids.length === 2) break;
    }
    await collectRaw;
    rawReader.cancel().catch(() => undefined);
    expect(ids).toEqual(all.slice(-2).map((e) => String(e.seq)));
    expect(raw.join('')).toContain(': heartbeat');
  });
});
