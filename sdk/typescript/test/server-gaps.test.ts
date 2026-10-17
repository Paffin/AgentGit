// Executable notes on the server contract the SDK depends on.
//
// "fixed" pins server behaviour that used to differ from docs/protocol and that the SDK now relies on
// (the SDK has no workaround for any of it). "remaining" pins what still differs or is surprising; a
// failure there is good news - the server changed: update the guide ("What the SDKs work around") and the
// SDK workaround, if any.

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { blobObject, generateAgentKey, putFile, type AgentGitClient } from '../src/index.js';
import { REPO_ROOT, startServer, type RunningServer } from './harness/server.js';

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
const key = { idempotencyKey: 'same-key' };

describe('fixed on the server', () => {
  it('seven creating operations honour Idempotency-Key: a repeat returns the recorded response', async () => {
    const a = await admin.repos.create({ name: 'gaps/keyed' }, key);
    const b = await admin.repos.create({ name: 'gaps/keyed' }, key);
    expect(b.created_at_ms).toBe(a.created_at_ms);

    const t1 = await admin.tasks.create({ repo, title: 'once' }, key);
    const t2 = await admin.tasks.create({ repo, title: 'once' }, key);
    expect(t2.id).toBe(t1.id);
    expect((await admin.tasks.list({ repo })).tasks.filter((t) => t.title === 'once')).toHaveLength(1);
    // the same key with different content is refused, not replayed
    await expect(admin.tasks.create({ repo, title: 'another' }, key)).rejects.toMatchObject({
      code: 'idempotency_key_reused',
    });

    const bot = await server.actor('agent:gap-bot', repo, 'writer', { retry: false });
    await bot.agents.register({ repo, name: 'gap-bot', public_key: generateAgentKey().publicKeyHex });
    const r1 = await bot.agents.startRun({ repo, agent: 'gap-bot' }, key);
    const r2 = await bot.agents.startRun({ repo, agent: 'gap-bot' }, key);
    expect(r2.run_id).toBe(r1.run_id);
    expect((await bot.agents.listRuns({ repo, agent: 'gap-bot' })).runs).toHaveLength(1);

    const l1 = await bot.leases.acquire({ repo, scope: 'gap/**', exclusive: true, ttl_ms: 60_000 }, key);
    const l2 = await bot.leases.acquire({ repo, scope: 'gap/**', exclusive: true, ttl_ms: 60_000 }, key);
    expect(l2.lease.id).toBe(l1.lease.id);
    expect(l2.lease.fencing_token).toBe(l1.lease.fencing_token);
  });

  it('GET /v1/capabilities takes list-valued parameters as repeated keys (the comma form is not a list)', async () => {
    const get = async (query: string) => {
      const res = await fetch(`${server.baseUrl}/v1/capabilities?${query}`);
      return { status: res.status, body: (await res.json()) as { protocol_version?: string; features?: string[] } };
    };
    const all = await get('');
    expect(all.body.features!.length).toBeGreaterThan(0);
    // the requested features are intersected with the server's
    const both = await get(`supported_versions=agwp.v1&features=${all.body.features![0]}&features=nope`);
    expect(both.status).toBe(200);
    expect(both.body).toMatchObject({ protocol_version: 'agwp.v1', features: [all.body.features![0]] });
    // a list of versions: the server picks the one it speaks
    expect((await get('supported_versions=agwp.v0&supported_versions=agwp.v1')).status).toBe(200);
    expect((await get('supported_versions=agwp.v0')).status).toBe(412);
    // "a,b" is one (unknown) feature, not two
    expect((await get(`features=${all.body.features![0]},nope`)).body.features).toEqual([]);
  });

  it('events.replay beyond the end returns nothing and the real latest_seq, u64 max included', async () => {
    const latest = (await admin.events.replay({ repo, after_seq: 0, limit: 1 })).latest_seq;
    expect(latest).toBeGreaterThan(0);
    const max = await admin.events.replay({ repo, after_seq: 18446744073709551615n as unknown as number, limit: 1 });
    expect(max.events).toHaveLength(0);
    expect(max.latest_seq).toBe(latest);
    const big = await admin.events.replay({ repo, after_seq: Number.MAX_SAFE_INTEGER, limit: 1 });
    expect(big.events).toHaveLength(0);
    expect(big.latest_seq).toBe(latest);
  });

  it('queue entries finish as state "done" with outcome "integrated"', async () => {
    await admin.workspaces.create({ repo, name: 'q', stream: 'main' });
    const change = await admin.workspaces.modify({ repo, name: 'q', changes: [putFile('q.txt', 'q')] });
    const p = await admin.proposals.create({ repo, stream: 'main', head: change.head, title: 'q' });
    await admin.queue.enqueue({ repo, proposal: p.id });
    const ran = await admin.queue.run({ repo, stream: 'main' });
    expect(ran.processed[0]).toMatchObject({ state: 'done', outcome: 'integrated' });
  });

  it('tree and directory listings carry real sizes', async () => {
    const listed = await admin.tree.get({ repo, rev: 'main' });
    expect(listed.entries.find((e) => e.path === 'q.txt')?.size).toBe(1);
    expect((await admin.tree.list({ repo, rev: 'main' })).entries[0]?.size).toBe(1);
    expect((await admin.tree.resolve({ repo, rev: 'main', path: 'q.txt' })).entry?.size).toBe(1);
  });

  it("a wrong method or an unknown route answers in the envelope, with the caller's request id", async () => {
    const wrong = await fetch(`${server.baseUrl}/v1/repos/${repo}`, {
      method: 'DELETE',
      headers: { ...auth(), 'x-request-id': 'gap-405' },
    });
    expect(wrong.status).toBe(405);
    expect(wrong.headers.get('allow')).not.toBeNull();
    expect(((await wrong.json()) as { error: { code: string; request_id: string } }).error).toMatchObject({
      code: 'invalid_argument',
      request_id: 'gap-405',
    });
    const unknown = await fetch(`${server.baseUrl}/v1/nope`, { headers: { ...auth(), 'x-request-id': 'gap-404' } });
    expect(unknown.status).toBe(404);
    expect(((await unknown.json()) as { error: { code: string; request_id: string } }).error).toMatchObject({
      code: 'not_found',
      request_id: 'gap-404',
    });
  });

  it('file_type is "regular" on reads and writes; the old spelling "file" is still accepted when writing', async () => {
    const cs = await admin.history.changeset({ repo, rev: 'main', include_ops: true });
    expect(cs.ops[0]?.file_type).toBe('regular');
    for (const [path, file_type] of [
      ['t-regular', 'regular'],
      ['t-alias', 'file'],
    ] as const) {
      const res = await admin.workspaces.modify({
        repo,
        name: 'q',
        changes: [{ path, op: 'put', content: 'eg==', file_type }],
      });
      expect(res.head).toMatch(/^[0-9a-f]{64}$/);
    }
    // the `kind` of an entry is a different vocabulary: file | symlink | dir
    expect((await admin.tree.resolve({ repo, rev: '@q', path: 't-regular' })).kind).toBe('file');
  });
});

describe('remaining', () => {
  it('stream.create and credentials.mint ignore Idempotency-Key, by design', async () => {
    // a repeated stream.create answers already_exists ...
    await admin.streams.create({ repo, name: 'dev' }, key);
    await expect(admin.streams.create({ repo, name: 'dev' }, key)).rejects.toMatchObject({ code: 'already_exists' });
    // ... and a credential's secret can be shown once, so a repeated mint is a second credential
    const c1 = await admin.credentials.mint({ subject: 'user:x', ttl_secs: 60 }, key);
    const c2 = await admin.credentials.mint({ subject: 'user:x', ttl_secs: 60 }, key);
    expect(c2.credential.id).not.toBe(c1.credential.id);
  });

  it('objects.put answers JSON, although OpenAPI lists application/octet-stream for its 200', async () => {
    const spec = JSON.parse(readFileSync(path.join(REPO_ROOT, 'docs/protocol/openapi.json'), 'utf8')) as {
      paths: Record<string, Record<string, { operationId: string; responses: Record<string, { content?: object }> }>>;
    };
    const put = Object.values(spec.paths)
      .flatMap((p) => Object.values(p))
      .find((o) => o.operationId === 'objects.put')!;
    expect(Object.keys(put.responses['200']!.content!)).toEqual(['application/octet-stream']);

    const blob = blobObject('gap\n');
    const res = await fetch(`${server.baseUrl}/v1/repos/${repo}/objects/${blob.id}`, {
      method: 'PUT',
      headers: { ...auth(), 'content-type': 'application/octet-stream' },
      body: blob.envelope as BodyInit,
    });
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toMatch(/^application\/json/);
    expect(await res.json()).toMatchObject({ object_id: blob.id });
  });

  it('Retry-After is whole seconds and at least 1, while retry_after_ms can be shorter', async () => {
    const limited = await startServer({ config: '[limits]\nrequests_per_second = 20\nburst = 2\n' });
    try {
      // a concurrent burst, so that a slow machine cannot let the bucket refill between requests
      const replies = await Promise.all(
        Array.from({ length: 40 }, async () => {
          const res = await fetch(`${limited.baseUrl}/v1/whoami`, {
            headers: { authorization: `Bearer ${limited.bootstrapToken}` },
          });
          return { status: res.status, header: res.headers.get('retry-after'), body: (await res.json()) as unknown };
        }),
      );
      const throttled = replies.find((r) => r.status === 429);
      expect(throttled).toBeDefined();
      const ms = (throttled!.body as { error: { retry_after_ms: number } }).error.retry_after_ms;
      expect(throttled!.header).toBe('1');
      expect(ms).toBeGreaterThan(0);
      expect(ms).toBeLessThan(1000); // the SDK waits for this, not for the rounded header
    } finally {
      await limited.stop();
    }
  });
});
