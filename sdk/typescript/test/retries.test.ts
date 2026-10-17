// Retries, idempotency keys and deadlines against a real server, with faults injected between the
// client and the server: a lost response must never apply a request twice.

import { createServer, type AddressInfo, type Server, type Socket } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AgentGitClient, AgentGitError, generateAgentKey, putFile, type AgentGitClientOptions } from '../src/index.js';
import { FaultyNetwork, recordedSleep } from './harness/faults.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;

beforeAll(async () => {
  server = await startServer();
  await server.admin().repos.create({ name: 'acme/retry' });
});
afterAll(async () => {
  await server?.stop();
});

const repo = 'acme/retry';

function setup(options: Partial<AgentGitClientOptions> = {}) {
  const net = new FaultyNetwork();
  const { sleep, delays } = recordedSleep();
  const client = new AgentGitClient({
    baseUrl: server.baseUrl,
    token: server.bootstrapToken,
    fetch: net.fetch,
    sleep,
    random: () => 0.5,
    ...options,
  });
  return { net, delays, client };
}

const unavailable = (retryAfterMs: number) => ({
  status: 503,
  headers: { 'content-type': 'application/json', ...(retryAfterMs > 0 ? { 'retry-after': '2' } : {}) },
  body: JSON.stringify({
    error: {
      code: 'unavailable',
      message: 'busy',
      retryable: true,
      retry_after_ms: retryAfterMs,
      request_id: 'req_x',
      details: {},
    },
  }),
});

describe('which requests are retried', () => {
  it('retries reads on retryable errors and honours the server-suggested delay', async () => {
    const { net, delays, client } = setup();
    net.rule = (_req, n) => (n <= 2 ? unavailable(120) : 'pass');
    const info = await client.repos.describe({ repo });
    expect(info.name).toBe(repo);
    expect(net.seen).toHaveLength(3);
    // retry_after_ms (from the body) wins over the coarser Retry-After header and is a floor;
    // jitter (random() = 0.5 of the 200 ms / 400 ms window) is added on top
    expect(delays).toEqual([120 + 100, 120 + 200]);
    // the same correlation id is used, suffixed per retry
    const ids = net.seen.map((r) => r.headers.get('x-request-id')!);
    expect(ids[1]).toBe(`${ids[0]}-r2`);
  });

  it('uses the Retry-After header when the body has no delay, and backs off exponentially otherwise', async () => {
    const a = setup();
    a.net.rule = (_r, n) =>
      n === 1 ? { status: 503, headers: { 'retry-after': '2' }, body: 'Service Unavailable' } : 'pass';
    await a.client.repos.describe({ repo });
    expect(a.delays).toEqual([2000 + 100]);

    const b = setup({ retry: { maxAttempts: 5, baseDelayMs: 100, maxDelayMs: 350 } });
    b.net.rule = (_r, n) => (n <= 4 ? unavailable(0) : 'pass');
    await b.client.repos.describe({ repo });
    // full jitter with a fixed random() = 0.5 of min(max, base * 2^(attempt-1))
    expect(b.delays).toEqual([50, 100, 175, 175]);
  });

  it('gives up after maxAttempts and reports how many attempts were made', async () => {
    const { net, client } = setup({ retry: { maxAttempts: 3 } });
    net.rule = () => unavailable(1);
    const err = (await client.repos.describe({ repo }).catch((e: unknown) => e)) as AgentGitError;
    expect(err).toMatchObject({ code: 'unavailable', status: 503, retryable: true, attempts: 3 });
    expect(net.seen).toHaveLength(3);
  });

  it('does not retry what the server did not mark retryable', async () => {
    const { net, client } = setup();
    for (const [call, code] of [
      [() => client.repos.describe({ repo: 'acme/nope' }), 'not_found'],
      [() => client.repos.create({ name: repo }), 'already_exists'],
      [() => client.repos.create({ name: 'Bad Name' }), 'invalid_argument'],
    ] as const) {
      net.seen.length = 0;
      await expect(call()).rejects.toMatchObject({ code, attempts: 1 });
      expect(net.seen).toHaveLength(1);
    }
  });

  it('does not retry a lost compare-and-swap: stream_moved needs a fresh read, not the same request', async () => {
    const { net, client } = setup();
    await client.streams.create({ repo, name: 'cas' });
    await client.workspaces.create({ repo, name: 'cas-ws', stream: 'cas' });
    const ws = await client.workspaces.modify({ repo, name: 'cas-ws', changes: [putFile('c.txt', 'c')] });
    await client.proposals.create({ repo, stream: 'cas', head: ws.head, title: 'publishes the head' });
    net.seen.length = 0;
    const err = (await client.streams
      .cas({ repo, stream: 'cas', expected_version: 7, expected_head: '', new_head: ws.head })
      .catch((e: unknown) => e)) as AgentGitError;
    // The server calls this retryable, but the identical request can never win: it is not repeated.
    expect(err).toMatchObject({ code: 'stream_moved', status: 409, retryable: true, attempts: 1 });
    expect(net.seen).toHaveLength(1);
  });

  it('refuses to wait for an absurd Retry-After instead of hanging', async () => {
    const { net, client } = setup({ retry: { maxRetryAfterMs: 5_000 } });
    net.rule = () => ({
      status: 429,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        error: { code: 'rate_limited', message: 'slow', retryable: true, retry_after_ms: 3_600_000 },
      }),
    });
    await expect(client.repos.describe({ repo })).rejects.toMatchObject({
      code: 'rate_limited',
      retryAfterMs: 3_600_000,
      attempts: 1,
    });
    expect(net.seen).toHaveLength(1);
  });
});

describe('idempotency keys', () => {
  it('generates a key for every keyed mutation and none for reads', async () => {
    const { net, client } = setup();
    await client.repos.describe({ repo });
    await client.repos.create({ name: 'acme/keys-1' });
    await client.repos.create({ name: 'acme/keys-2' });
    const [read, a, b] = net.seen;
    expect(read!.headers.get('idempotency-key')).toBeNull();
    expect(a!.headers.get('idempotency-key')).toMatch(/^sdk-[A-Za-z0-9]{24}$/);
    expect(b!.headers.get('idempotency-key')).not.toBe(a!.headers.get('idempotency-key'));
  });

  it('an explicit key (option or request field) is used as given', async () => {
    const { net, client } = setup();
    await client.repos.create({ name: 'acme/keys-3' }, { idempotencyKey: 'mine-1' });
    await client.repos.create({ name: 'acme/keys-4', idempotency_key: 'mine-2' });
    expect(net.seen.map((r) => r.headers.get('idempotency-key'))).toEqual(['mine-1', 'mine-2']);
  });

  it('a key reused for a different request is refused and never retried', async () => {
    const { net, client } = setup();
    await client.repos.create({ name: 'acme/keys-5' }, { idempotencyKey: 'dup-key' });
    net.seen.length = 0;
    await expect(client.repos.create({ name: 'acme/keys-6' }, { idempotencyKey: 'dup-key' })).rejects.toMatchObject({
      code: 'idempotency_key_reused',
      status: 422,
      retryable: false,
    });
    expect(net.seen).toHaveLength(1);
  });

  it('repo.create: a lost response is retried with the SAME key and the repository exists exactly once', async () => {
    const { net, client } = setup();
    net.match = (r) => r.method === 'POST' && r.path === '/v1/repos';
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    const created = await client.repos.create({ name: 'acme/lost-1' });
    expect(created.name).toBe('acme/lost-1');
    const posts = net.to('/v1/repos', 'POST');
    expect(posts).toHaveLength(2);
    expect(posts[1]!.headers.get('idempotency-key')).toBe(posts[0]!.headers.get('idempotency-key'));
    expect(posts[1]!.body).toBe(posts[0]!.body);
    const listed = await client.repos.list({ prefix: 'acme/lost-1' });
    expect(listed.repos).toHaveLength(1);
  });

  it('proposal.create and integration.integrate: lost responses never duplicate the effect', async () => {
    const { net } = setup();
    const alice = await server.actor('user:alice', repo, 'writer', { fetch: net.fetch, sleep: async () => undefined });
    const carol = await server.actor('user:carol', repo, 'integrator', {
      fetch: net.fetch,
      sleep: async () => undefined,
    });
    const ws = await alice.workspaces.create({ repo, name: 'ws', stream: 'main' });
    expect(ws.name).toBe('ws');
    const changed = await alice.workspaces.modify({ repo, name: 'ws', changes: [putFile('a.txt', 'a\n')] });

    net.match = (r) => r.method === 'POST' && /\/proposals$/.test(r.path);
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    const proposal = await alice.proposals.create({ repo, stream: 'main', head: changed.head, title: 'lost response' });
    expect(
      (await alice.proposals.list({ repo, state: 'open' })).proposals.filter((p) => p.id === proposal.id),
    ).toHaveLength(1);
    expect((await alice.proposals.list({ repo })).proposals.filter((p) => p.title === 'lost response')).toHaveLength(1);

    net.reset();
    net.match = (r) => r.method === 'POST' && /\/integrate$/.test(r.path);
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    const done = await carol.integration.integrate({ repo, proposal: proposal.id });
    expect(done.outcome.status).toBe('integrated');
    const posts = net.seen.filter((r) => /\/integrate$/.test(r.path));
    expect(posts).toHaveLength(2);
    expect(posts[0]!.headers.get('idempotency-key')).toBe(posts[1]!.headers.get('idempotency-key'));
    // one integration, one reflog entry
    const reflog = await carol.streams.reflog({ repo, stream: 'main' });
    expect(reflog.entries.filter((e) => e.new_head === changed.head)).toHaveLength(1);
    expect((await carol.streams.head({ repo, stream: 'main' })).version).toBe(1);
  });

  it('task.create, agent.start_run and lease.acquire: a lost response is retried with the SAME key and the effect happens once', async () => {
    const { net } = setup();
    const bot = await server.actor('agent:retry-bot', repo, 'writer', {
      fetch: net.fetch,
      sleep: async () => undefined,
    });
    await bot.agents.register({ repo, name: 'retry-bot', public_key: generateAgentKey().publicKeyHex });
    net.reset();
    const lostOnce = (path: RegExp) => {
      net.reset();
      net.match = (r) => r.method === 'POST' && path.test(r.path);
      net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    };
    const sameKeyTwice = (path: RegExp) => {
      const posts = net.seen.filter((r) => r.method === 'POST' && path.test(r.path));
      expect(posts).toHaveLength(2);
      expect(posts[0]!.headers.get('idempotency-key')).toMatch(/^sdk-/);
      expect(posts[1]!.headers.get('idempotency-key')).toBe(posts[0]!.headers.get('idempotency-key'));
      expect(posts[1]!.body).toBe(posts[0]!.body);
    };

    lostOnce(/\/tasks$/);
    const task = await bot.tasks.create({ repo, title: 'created once' });
    sameKeyTwice(/\/tasks$/);
    const tasks = (await bot.tasks.list({ repo })).tasks.filter((t) => t.title === 'created once');
    expect(tasks.map((t) => t.id)).toEqual([task.id]);

    lostOnce(/\/runs$/);
    const run = await bot.agents.startRun({ repo, agent: 'retry-bot', task_id: task.id });
    sameKeyTwice(/\/runs$/);
    const runs = (await bot.agents.listRuns({ repo, agent: 'retry-bot' })).runs;
    expect(runs.map((r) => r.run_id)).toEqual([run.run_id]);

    lostOnce(/\/leases$/);
    const lease = await bot.leases.acquire({ repo, scope: 'retry/**', exclusive: true, ttl_ms: 60_000 });
    sameKeyTwice(/\/leases$/);
    // the retry did not collide with the first attempt's own lease, and only one lease exists
    const leases = (await bot.leases.list({ repo, active_only: true })).leases.filter((l) => l.scope === 'retry/**');
    expect(leases.map((l) => l.id)).toEqual([lease.lease.id]);
  });

  it('objects.put is naturally idempotent and is retried after a lost response', async () => {
    const { net, client } = setup();
    net.match = (r) => r.method === 'PUT';
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    const res = await client.objects.putBlob({ repo, content: 'lost put\n' });
    expect(net.seen.filter((r) => r.method === 'PUT')).toHaveLength(2);
    expect(res.stored).toBe(false); // the first attempt had already stored it
  });
});

describe('requests the server cannot deduplicate', () => {
  // credentials.mint ignores Idempotency-Key by design (the secret can only be shown once), so a repeat
  // would mint a second credential.
  const minted = async (client: AgentGitClient, subject: string) =>
    (await client.credentials.list({ subject })).credentials.filter((c) => c.subject === subject);

  it('are NOT retried after a lost response: the error says the outcome is unknown', async () => {
    const { net, client } = setup();
    net.match = (r) => r.method === 'POST' && r.path === '/v1/credentials';
    net.rule = () => 'drop-response';
    const err = (await client.credentials
      .mint({ subject: 'user:maybe-twice', ttl_secs: 60 })
      .catch((e: unknown) => e)) as AgentGitError;
    expect(err).toBeInstanceOf(AgentGitError);
    expect(err).toMatchObject({ code: 'unavailable', status: 0, retryable: true, outcomeUnknown: true, attempts: 1 });
    expect(net.to('/v1/credentials', 'POST')).toHaveLength(1);
    // The server did apply it once.
    expect(await minted(client, 'user:maybe-twice')).toHaveLength(1);
  });

  it('are retried only on request, and then they can be applied twice - which is why that is off by default', async () => {
    const { net, client } = setup();
    net.match = (r) => r.method === 'POST' && r.path === '/v1/credentials';
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    await client.credentials.mint({ subject: 'user:forced-retry', ttl_secs: 60 }, { retry: { retryUnsafe: true } });
    expect(await minted(client, 'user:forced-retry')).toHaveLength(2);
  });

  it('workspace.modify becomes retryable once it carries expected_head: a second application is refused', async () => {
    const { net } = setup();
    const alice = await server.actor('user:wally', repo, 'writer', { fetch: net.fetch, sleep: async () => undefined });
    const ws = await alice.workspaces.create({ repo, name: 'guarded', stream: 'main' });
    net.match = (r) => r.method === 'POST' && r.path.endsWith('/guarded/changes');
    net.reset();
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    // Without the guard a lost response is not retried ...
    const unguarded = await alice.workspaces
      .modify({ repo, name: 'guarded', changes: [putFile('x', '1')] })
      .catch((e: unknown) => e);
    expect(unguarded).toMatchObject({ outcomeUnknown: true, attempts: 1 });
    // ... with it, the retry runs (and is refused, because the first attempt already moved the head).
    net.reset();
    net.rule = (_r, n) => (n === 1 ? 'drop-response' : 'pass');
    const current = (await alice.workspaces.get({ repo, name: 'guarded' })).head;
    expect(current).not.toBe(ws.head);
    const guarded = await alice.workspaces
      .modify({ repo, name: 'guarded', changes: [putFile('y', '2')], expected_head: current })
      .catch((e: unknown) => e);
    expect(guarded).toMatchObject({ code: 'failed_precondition', attempts: 2 });
    // exactly one of the two y-writes landed
    const tree = await alice.tree.get({ repo, rev: '@guarded' });
    expect(tree.entries.filter((e) => e.path === 'y')).toHaveLength(1);
  });
});

describe('deadlines', () => {
  it('sends the deadline to the server, which answers deadline_exceeded without waiting for the long-poll', async () => {
    const { net, client } = setup({ retry: false });
    const started = Date.now();
    const err = (await client.streams
      .head({ repo, stream: 'main', wait_version_gt: 9999, wait_ms: 20_000 }, { timeoutMs: 300 })
      .catch((e: unknown) => e)) as AgentGitError;
    expect(Date.now() - started).toBeLessThan(5000);
    expect(net.seen[0]!.headers.get('request-timeout')).toBe('300');
    expect(err).toMatchObject({ code: 'deadline_exceeded', status: 504, retryable: true, outcomeUnknown: true });
  });

  it('retries a deadline for reads (they are safe to repeat) and reports each attempt', async () => {
    const { net, client } = setup({ retry: { maxAttempts: 2 } });
    const err = (await client.streams
      .head({ repo, stream: 'main', wait_version_gt: 9999, wait_ms: 20_000 }, { timeoutMs: 200 })
      .catch((e: unknown) => e)) as AgentGitError;
    expect(err.attempts).toBe(2);
    expect(net.seen).toHaveLength(2);
  });

  it('a long-poll gets a deadline longer than its wait automatically', async () => {
    const { net, client } = setup();
    const head = await client.streams.head({ repo, stream: 'main', wait_version_gt: 9999, wait_ms: 150 });
    expect(head.name).toBe('main');
    expect(Number(net.seen[0]!.headers.get('request-timeout'))).toBeGreaterThan(10_000);
  });

  it('enforces the deadline locally when the server never answers', async () => {
    const sockets = new Set<Socket>();
    const silent: Server = createServer((s) => {
      sockets.add(s);
      s.on('error', () => undefined);
    });
    await new Promise<void>((resolve) => silent.listen(0, '127.0.0.1', resolve));
    try {
      const port = (silent.address() as AddressInfo).port;
      const client = new AgentGitClient({ baseUrl: `http://127.0.0.1:${port}`, token: 'x', retry: false });
      const started = Date.now();
      const err = (await client.session.whoami({}, { timeoutMs: 250 }).catch((e: unknown) => e)) as AgentGitError;
      expect(Date.now() - started).toBeLessThan(3000);
      expect(err).toMatchObject({ code: 'deadline_exceeded', status: 0, retryable: true, outcomeUnknown: true });
    } finally {
      for (const s of sockets) s.destroy();
      await new Promise<void>((resolve) => silent.close(() => resolve()));
    }
  });

  it('an AbortSignal from the caller cancels the call and its retries, with the signal reason', async () => {
    const { net } = setup();
    net.rule = () => unavailable(1);
    const abort = new AbortController();
    const slow = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      fetch: net.fetch,
      sleep: (_ms, signal) => {
        // The caller gives up while the client waits to retry.
        abort.abort(new Error('stop it'));
        return Promise.reject(signal?.reason as Error);
      },
    });
    await expect(slow.repos.describe({ repo }, { signal: abort.signal })).rejects.toThrow('stop it');
    expect(net.seen).toHaveLength(1);
  });

  it('a signal that is already aborted never reaches the network', async () => {
    const { net, client } = setup();
    await expect(client.repos.describe({ repo }, { signal: AbortSignal.abort(new Error('nope')) })).rejects.toThrow(
      'nope',
    );
    expect(net.seen.length).toBeLessThanOrEqual(1);
  });
});
