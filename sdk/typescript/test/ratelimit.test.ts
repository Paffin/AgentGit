// Backpressure: the server rate-limits per credential and says how long to wait (429 + retry_after_ms).

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AgentGitClient, AgentGitError } from '../src/index.js';
import { FaultyNetwork } from './harness/faults.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
const repo = 'acme/limited';

beforeAll(async () => {
  server = await startServer({ config: '[limits]\nrequests_per_second = 20\nburst = 5\n' });
  const admin = server.admin();
  await admin.repos.create({ name: repo });
  await admin.repos.create({ name: 'acme/small' });
});
afterAll(async () => {
  await server?.stop();
});

describe('429 rate_limited', () => {
  it('without retries a burst surfaces rate_limited with a retry hint', async () => {
    const reader = await server.actor('user:burst', repo, 'reader', { retry: false });
    let limited: AgentGitError | undefined;
    for (let i = 0; i < 30 && !limited; i++) {
      limited = (await reader.repos.describe({ repo }).catch((e: unknown) => e)) as AgentGitError | undefined;
      if (!(limited instanceof AgentGitError)) limited = undefined;
    }
    expect(limited).toBeDefined();
    expect(limited).toMatchObject({ code: 'rate_limited', status: 429, retryable: true });
    expect(limited!.retryAfterMs).toBeGreaterThan(0);
    expect(limited!.retryAfterMs).toBeLessThanOrEqual(1000);
  });

  it('with the default retry policy the client waits as told and every request succeeds', async () => {
    const net = new FaultyNetwork();
    const waits: number[] = [];
    const reader = await server.actor('user:patient', repo, 'reader', {
      fetch: net.fetch,
      retry: { maxAttempts: 10 },
      sleep: async (ms) => {
        waits.push(ms);
        await new Promise((r) => setTimeout(r, ms));
      },
    });
    const started = Date.now();
    const results = await Promise.all(Array.from({ length: 16 }, () => reader.repos.describe({ repo })));
    expect(results.every((r) => r.name === repo)).toBe(true);
    // 16 requests, burst 5, 20 per second: at least some had to be told to wait, and the client did.
    expect(waits.length).toBeGreaterThan(0);
    expect(Date.now() - started).toBeGreaterThan(500);
    const throttled = net.seen.length - 16;
    expect(throttled).toBeGreaterThan(0);
  });

  it('a mutation the server cannot deduplicate is still retried on 429 - it was rejected before it ran - and applied once', async () => {
    const writer = await server.actor('user:tasker', repo, 'writer', { retry: { maxAttempts: 10 } });
    const titles = Array.from({ length: 12 }, (_, i) => `task ${i}`);
    await Promise.all(titles.map((title) => writer.tasks.create({ repo, title })));
    const tasks = (await writer.tasks.list({ repo })).tasks.map((t) => t.title).sort();
    expect(tasks).toEqual([...titles].sort());
  });

  it('an unsafe mutation hit by a non-429 transient error is not retried', async () => {
    const net = new FaultyNetwork();
    net.match = (r) => r.path.endsWith('/tasks') && r.method === 'POST';
    net.rule = () => ({
      status: 503,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ error: { code: 'unavailable', message: 'busy', retryable: true, retry_after_ms: 1 } }),
    });
    const client = new AgentGitClient({ baseUrl: server.baseUrl, token: server.bootstrapToken, fetch: net.fetch });
    await expect(client.tasks.create({ repo, title: 'no retry' })).rejects.toMatchObject({
      code: 'unavailable',
      attempts: 1,
    });
    expect(net.seen).toHaveLength(1);
  });

  it('exports the limiter to the caller: separate credentials have separate budgets', async () => {
    const a = await server.actor('user:a-budget', repo, 'reader', { retry: false });
    const b = await server.actor('user:b-budget', repo, 'reader', { retry: false });
    for (let i = 0; i < 40; i++) await a.repos.describe({ repo }).catch(() => undefined);
    // a is (probably) throttled now, b has a full burst
    for (let i = 0; i < 4; i++) await b.repos.describe({ repo });
  });
});

describe('quota_exceeded is a 429 that waiting does not fix', () => {
  it('is not retried', async () => {
    const admin = server.admin({ retry: false });
    await admin.repos.configure({ repo: 'acme/small', quota: { max_streams: 2 } });
    const writer = await server.actor('user:quota', 'acme/small', 'writer', { retry: { maxAttempts: 5 } });
    await writer.streams.create({ repo: 'acme/small', name: 'dev' });
    const err = (await writer.streams
      .create({ repo: 'acme/small', name: 'dev2' })
      .catch((e: unknown) => e)) as AgentGitError;
    expect(err).toMatchObject({ code: 'quota_exceeded', status: 429, retryable: false, attempts: 1 });
  });
});
