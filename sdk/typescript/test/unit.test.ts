// Pure client-side behaviour that needs no server: request building, SSE parsing, option handling.

import { describe, expect, it } from 'vitest';
import { AgentGitClient, AgentGitError, OPERATIONS, readSse, type SseFrame } from '../src/index.js';
import { buildRequest } from '../src/transport.js';
import { errorFromStatus } from '../src/errors.js';

const base = 'http://srv.test';

describe('building requests', () => {
  it('splits repo into path segments and sends the rest as a query for reads', () => {
    const b = buildRequest(
      OPERATIONS['history.log'],
      { repo: 'acme/web', rev: 'main~2', path: 'src/a b.rs', page_size: 5 },
      base,
    );
    expect(b.method).toBe('GET');
    const url = new URL(b.url);
    expect(url.pathname).toBe('/v1/repos/acme/web/log');
    expect(Object.fromEntries(url.searchParams)).toEqual({ rev: 'main~2', path: 'src/a b.rs', page_size: '5' });
    expect(b.json).toBeUndefined();
  });

  it('keeps free-form names in the query string or body, never in the path', () => {
    const b = buildRequest(OPERATIONS['stream.head'], { repo: 'acme/web', stream: 'feature/x y#1?', wait_ms: 5 }, base);
    expect(new URL(b.url).pathname).toBe('/v1/repos/acme/web/streams/head');
    expect(new URL(b.url).searchParams.get('stream')).toBe('feature/x y#1?');
  });

  it('encodes each segment of a file path and drops a leading slash', () => {
    const b = buildRequest(
      OPERATIONS['tree.file'],
      { repo: 'acme/web', path: '/dir one/файл #1?.txt', rev: 'main' },
      base,
    );
    expect(b.url).toBe(`${base}/v1/repos/acme/web/files/dir%20one/%D1%84%D0%B0%D0%B9%D0%BB%20%231%3F.txt?rev=main`);
  });

  it('joins topics with commas and repeats other arrays; booleans and undefined behave', () => {
    const r = buildRequest(
      OPERATIONS['events.replay'],
      { repo: 'acme/web', topics: ['stream', 'proposal.create'], after_seq: 0, limit: undefined },
      base,
    );
    expect(new URL(r.url).searchParams.get('topics')).toBe('stream,proposal.create');
    expect(new URL(r.url).searchParams.get('after_seq')).toBe('0');
    expect(new URL(r.url).searchParams.has('limit')).toBe(false);
    const n = buildRequest(OPERATIONS['session.negotiate'], { supported_versions: ['a', 'b'], client_name: 'x' }, base);
    expect(new URL(n.url).searchParams.getAll('supported_versions')).toEqual(['a', 'b']);
    const t = buildRequest(OPERATIONS['task.list'], { repo: 'acme/web', ready_only: true }, base);
    expect(new URL(t.url).searchParams.get('ready_only')).toBe('true');
  });

  it('sends body fields as JSON without the path parameters', () => {
    const b = buildRequest(
      OPERATIONS['proposal.review'],
      { repo: 'acme/web', id: 'prop_1', decision: 'approve', body: 'ok' },
      base,
    );
    expect(b.url).toBe(`${base}/v1/repos/acme/web/proposals/prop_1/reviews`);
    expect(JSON.parse(b.json!)).toEqual({ decision: 'approve', body: 'ok' });
    const g = buildRequest(
      OPERATIONS['admin.roles.grant'],
      { subject: 'user:a', repo: 'acme/*', role: 'reader' },
      base,
    );
    expect(g.url).toBe(`${base}/v1/roles`);
    expect(JSON.parse(g.json!)).toEqual({ subject: 'user:a', repo: 'acme/*', role: 'reader' }); // repo is a plain field here
  });

  it('DELETE routes take path and query only', () => {
    const b = buildRequest(
      OPERATIONS['admin.roles.revoke'],
      { subject: 'user:a', repo: 'acme/web', role: 'reader' },
      base,
    );
    expect(b.method).toBe('DELETE');
    expect(b.json).toBeUndefined();
    expect(new URL(b.url).searchParams.get('repo')).toBe('acme/web');
    expect(buildRequest(OPERATIONS['lease.release'], { repo: 'acme/web', lease_id: 'lease_1' }, base).url).toBe(
      `${base}/v1/repos/acme/web/leases/lease_1`,
    );
  });

  it('rejects nonsense early', () => {
    expect(() => buildRequest(OPERATIONS['repo.describe'], { repo: 'a/b/c' }, base)).toThrow(/owner\/name/);
    expect(() => buildRequest(OPERATIONS['repo.describe'], {}, base)).toThrow(/owner\/name/);
    expect(() => buildRequest(OPERATIONS['proposal.get'], { repo: 'a/b' }, base)).toThrow(/"id"/);
    expect(() => buildRequest(OPERATIONS['repo.list'], { prefix: { nested: 1 } }, base)).toThrow(/query string/);
  });
});

function streamOf(parts: (string | Uint8Array)[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(c) {
      for (const p of parts) c.enqueue(typeof p === 'string' ? enc.encode(p) : p);
      c.close();
    },
  });
}
async function frames(parts: (string | Uint8Array)[]): Promise<SseFrame[]> {
  const out: SseFrame[] = [];
  for await (const f of readSse(streamOf(parts))) out.push(f);
  return out;
}

describe('SSE parsing', () => {
  it('parses id, event and data, ignoring comments', async () => {
    const got = await frames([': heartbeat\n\n', 'id: 7\nevent: stream.integrate\ndata: {"seq":7}\n\n', ': hb\n\n']);
    expect(got).toEqual([{ event: 'stream.integrate', data: '{"seq":7}', id: '7' }]);
  });

  it('handles \\r\\n, lone \\r, a CRLF split across chunks, and multi-line data', async () => {
    expect(await frames(['id: 1\r\ndata: a\r\ndata: b\r\n\r\n'])).toEqual([
      { event: 'message', data: 'a\nb', id: '1' },
    ]);
    expect(await frames(['data: x\r\r'])).toEqual([{ event: 'message', data: 'x', id: '' }]);
    expect(await frames(['data: y\r', '\n\r', '\n'])).toEqual([{ event: 'message', data: 'y', id: '' }]);
  });

  it('survives chunk boundaries anywhere, including inside a multi-byte character', async () => {
    const bytes = new TextEncoder().encode('id: 3\nevent: x\ndata: {"t":"Привет ☃ 🦀"}\n\n');
    for (let cut = 1; cut < bytes.length; cut += 3) {
      const got = await frames([bytes.slice(0, cut), bytes.slice(cut)]);
      expect(got).toHaveLength(1);
      expect(JSON.parse(got[0]!.data)).toEqual({ t: 'Привет ☃ 🦀' });
    }
  });

  it('strips a BOM, keeps the last id across frames, reads retry, and drops an unterminated event', async () => {
    const got = await frames(['﻿id: 1\ndata: a\n\n', 'data: b\nretry: 1500\n\n', 'data: never finished']);
    expect(got).toEqual([
      { event: 'message', data: 'a', id: '1' },
      { event: 'message', data: 'b', id: '1', retry: 1500 },
    ]);
  });
});

describe('client options', () => {
  const echo =
    (calls: Request[]): typeof fetch =>
    async (input, init) => {
      calls.push(new Request(input, init));
      return new Response('{}', { headers: { 'content-type': 'application/json' } });
    };

  it('asks a token function for a fresh credential on every request', async () => {
    const calls: Request[] = [];
    let n = 0;
    const client = new AgentGitClient({ baseUrl: base, token: () => `tok-${++n}`, fetch: echo(calls) });
    await client.session.whoami();
    await client.session.whoami();
    expect(calls.map((r) => r.headers.get('authorization'))).toEqual(['Bearer tok-1', 'Bearer tok-2']);
  });

  it('keeps a path prefix of the base URL and trims trailing slashes', async () => {
    const calls: Request[] = [];
    const client = new AgentGitClient({ baseUrl: `${base}/agentgit//`, token: 't', fetch: echo(calls) });
    await client.repos.describe({ repo: 'a/b' });
    expect(calls[0]!.url).toBe(`${base}/agentgit/v1/repos/a/b`);
    expect(client.baseUrl).toBe(`${base}/agentgit`);
  });

  it('withRun and withToken return new clients and leave the original alone', async () => {
    const calls: Request[] = [];
    const a = new AgentGitClient({ baseUrl: base, token: 'a', fetch: echo(calls) });
    const b = a.withRun('run_1').withToken('b');
    await a.session.whoami();
    await b.session.whoami();
    expect(calls[0]!.headers.get('ag-run')).toBeNull();
    expect(calls[1]!.headers.get('ag-run')).toBe('run_1');
    expect(calls[1]!.headers.get('authorization')).toBe('Bearer b');
    await a.session.whoami({}, { runId: 'run_2', headers: { 'X-Extra': '1' } });
    expect(calls[2]!.headers.get('ag-run')).toBe('run_2');
    expect(calls[2]!.headers.get('x-extra')).toBe('1');
  });

  it('puts bytes on the wire as an octet stream for objects.put', async () => {
    const seen: { type: string | null; length: number }[] = [];
    const client = new AgentGitClient({
      baseUrl: base,
      token: 't',
      fetch: async (_i, init) => {
        seen.push({ type: new Headers(init?.headers).get('content-type'), length: (init?.body as Uint8Array).length });
        return new Response('{"object_id":"x","stored":true}', { headers: { 'content-type': 'application/json' } });
      },
    });
    await client.objects.put({ repo: 'a/b', object_id: 'x', data: new Uint8Array(5) });
    expect(seen).toEqual([{ type: 'application/octet-stream', length: 5 }]);
    // @ts-expect-error - data must be bytes
    expect(() => client.objects.put({ repo: 'a/b', object_id: 'x', data: 'text' })).toThrow(/Uint8Array/);
  });

  it('refuses to be built without a base URL or a fetch', () => {
    // @ts-expect-error - baseUrl is required
    expect(() => new AgentGitClient({})).toThrow(/baseUrl/);
    const saved = globalThis.fetch;
    try {
      // @ts-expect-error - simulating an old runtime
      delete globalThis.fetch;
      expect(() => new AgentGitClient({ baseUrl: base })).toThrow(/fetch/);
    } finally {
      globalThis.fetch = saved;
    }
  });

  it('negotiate does not send version arrays (ag-server 0.1.0 rejects repeated query parameters)', async () => {
    const calls: Request[] = [];
    const client = new AgentGitClient({
      baseUrl: base,
      fetch: async (input, init) => {
        calls.push(new Request(input, init));
        return new Response('{"protocol_version":"agwp.v1","features":[],"auth_methods":[]}', {
          headers: { 'content-type': 'application/json' },
        });
      },
    });
    await client.negotiate({ clientName: 'x', clientVersion: '1' });
    expect(new URL(calls[0]!.url).search).toBe('?client_name=x&client_version=1');
    expect(calls[0]!.headers.get('authorization')).toBeNull();
  });
});

describe('errors without an envelope', () => {
  it('maps HTTP statuses to codes and marks transient ones retryable', () => {
    const e = (s: number) => errorFromStatus(s, '', {});
    expect(e(502)).toMatchObject({ code: 'unavailable', retryable: true, outcomeUnknown: true });
    expect(e(504)).toMatchObject({ code: 'deadline_exceeded', retryable: true });
    expect(e(429)).toMatchObject({ code: 'rate_limited', retryable: true });
    expect(e(404)).toMatchObject({ code: 'not_found', retryable: false });
    expect(e(405)).toMatchObject({ code: 'http_error', retryable: false });
    expect(e(500)).toMatchObject({ code: 'internal', retryable: false });
    expect(errorFromStatus(502, '<html>Bad Gateway</html>', {}).message).toContain('Bad Gateway');
  });

  it('an AgentGitError is a real Error with a useful name, stack and cause', () => {
    const cause = new Error('boom');
    const err = new AgentGitError({ code: 'internal', message: 'x', cause });
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe('AgentGitError');
    expect(err.cause).toBe(cause);
    expect(err.stack).toContain('AgentGitError');
    expect(err.attempts).toBe(1);
  });
});
