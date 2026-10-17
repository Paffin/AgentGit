// Executable notes on where ag-server 0.1.0 differs from what docs/protocol and agwp.proto promise.
// The SDK is written around these. A failure here is good news: the server changed - update the
// "Things the server does today that show through" section of the README and the SDK workarounds.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { putFile, type AgentGitClient } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
let admin: AgentGitClient;
const repo = 'gaps/one';

beforeAll(async () => {
  server = await startServer();
  admin = server.admin({ retry: false });
  await admin.repos.create({ name: repo });
});
afterAll(async () => {
  await server?.stop();
});

const auth = () => ({ authorization: `Bearer ${server.bootstrapToken}` });

describe('known gaps', () => {
  it('only four operations honour Idempotency-Key', async () => {
    const key = { idempotencyKey: 'same-key' };
    // honoured: the second call returns the recorded result
    const a = await admin.repos.create({ name: 'gaps/keyed' }, key);
    const b = await admin.repos.create({ name: 'gaps/keyed' }, key);
    expect(b.created_at_ms).toBe(a.created_at_ms);
    // ignored: the fields exist in the messages and the OpenAPI document, but nothing is recorded
    await admin.streams.create({ repo, name: 'dev' }, key);
    await expect(admin.streams.create({ repo, name: 'dev' }, key)).rejects.toMatchObject({ code: 'already_exists' });
    const t1 = await admin.tasks.create({ repo, title: 'dup' }, key);
    const t2 = await admin.tasks.create({ repo, title: 'dup' }, key);
    expect(t2.id).not.toBe(t1.id);
    const c1 = await admin.credentials.mint({ subject: 'user:x', ttl_secs: 60 }, key);
    const c2 = await admin.credentials.mint({ subject: 'user:x', ttl_secs: 60 }, key);
    expect(c2.credential.id).not.toBe(c1.credential.id);
  });

  it('GET /v1/capabilities cannot take the array parameters its OpenAPI entry lists', async () => {
    for (const query of ['supported_versions=agwp.v1', 'features=a&features=b', 'features=a,b']) {
      const res = await fetch(`${server.baseUrl}/v1/capabilities?${query}`);
      expect(res.status, query).toBe(400);
    }
  });

  it('events.replay treats after_seq = u64::MAX as "from the start"', async () => {
    const max = await admin.events.replay({ repo, after_seq: 18446744073709551615n as unknown as number, limit: 1 });
    expect(max.events[0]?.seq).toBe(1);
    const big = await admin.events.replay({ repo, after_seq: Number.MAX_SAFE_INTEGER, limit: 1 });
    expect(big.events).toHaveLength(0);
    expect(big.latest_seq).toBeGreaterThan(0);
  });

  it('queue entries finish as state "done" with an outcome, not "integrated"', async () => {
    const ws = await admin.workspaces.create({ repo, name: 'q', stream: 'main' });
    expect(ws.name).toBe('q');
    const change = await admin.workspaces.modify({ repo, name: 'q', changes: [putFile('q.txt', 'q')] });
    const p = await admin.proposals.create({ repo, stream: 'main', head: change.head, title: 'q' });
    await admin.queue.enqueue({ repo, proposal: p.id });
    const ran = await admin.queue.run({ repo, stream: 'main' });
    expect(ran.processed[0]).toMatchObject({ state: 'done', outcome: 'integrated' });
  });

  it('tree listings report size 0 where tree.resolve knows the size', async () => {
    const listed = await admin.tree.get({ repo, rev: 'main' });
    const entry = listed.entries.find((e) => e.path === 'q.txt')!;
    const resolved = await admin.tree.resolve({ repo, rev: 'main', path: 'q.txt' });
    expect(entry.size).toBe(0);
    expect(resolved.entry?.size).toBe(1);
    expect((await admin.tree.list({ repo, rev: 'main' })).entries[0]?.size).toBe(0);
  });

  it('a 405 has no JSON body, and an unknown route has no request id', async () => {
    const res = await fetch(`${server.baseUrl}/v1/repos/${repo}`, { method: 'DELETE', headers: auth() });
    expect(res.status).toBe(405);
    expect(await res.text()).toBe('');
    const unknown = await fetch(`${server.baseUrl}/v1/nope`, { headers: auth() });
    expect(unknown.status).toBe(404);
    expect(((await unknown.json()) as { error: { request_id: string } }).error.request_id).toBe('');
  });

  it('file types are "file" on reads but "regular" on writes', async () => {
    const cs = await admin.history.changeset({ repo, rev: 'main', include_ops: true });
    expect(cs.ops[0]?.file_type).toBe('file');
    await expect(
      admin.workspaces.modify({
        repo,
        name: 'q',
        changes: [{ path: 'z', op: 'put', content: 'eg==', file_type: 'file' }],
      }),
    ).rejects.toMatchObject({ code: 'invalid_argument' });
  });
});
