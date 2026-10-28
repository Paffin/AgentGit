// A tour of EVERY operation of the HTTP API through the SDK, against a real server. A recording
// fetch maps each request back to its OpenAPI operation id; the final test fails if any
// operation of the OpenAPI document was not reached (or if the server answered "no such route" / 405 for one).

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  AgentGitClient,
  OPERATIONS,
  blobObject,
  buildChangeset,
  generateAgentKey,
  putFile,
  signAttestation,
  signProvenance,
  type AgentKey,
  type OperationId,
} from '../src/index.js';
import { NAMESPACE_METHODS } from '../src/generated/api.js';
import { REPO_ROOT, startServer, type RunningServer } from './harness/server.js';

interface Hit {
  id: OperationId;
  status: number;
  routeMissing: boolean;
}
const hits: Hit[] = [];

const matchers = (Object.values(OPERATIONS) as readonly (typeof OPERATIONS)[OperationId][]).map((op) => ({
  op,
  re: new RegExp('^' + op.route.replace(/\{\*[^}]+\}/g, '.+').replace(/\{[^}]+\}/g, '[^/]+') + '$'),
}));

const recording: typeof fetch = async (input, init) => {
  const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
  const method = (init?.method ?? 'GET').toUpperCase();
  const response = await fetch(input, init);
  const m = matchers.find((x) => x.op.http === method && x.re.test(url.pathname));
  if (m) {
    const body = response.ok ? '' : await response.clone().text();
    hits.push({
      id: m.op.id,
      status: response.status,
      routeMissing: response.status === 405 || body.includes('no such route'),
    });
  }
  return response;
};

let server: RunningServer;
const repo = 'tour/all';
const ciKey: AgentKey = generateAgentKey();
let admin: AgentGitClient;
let alice: AgentGitClient;
let bob: AgentGitClient;
let carol: AgentGitClient;
let ci: AgentGitClient;

beforeAll(async () => {
  server = await startServer();
  admin = server.admin({ fetch: recording });
  const opts = { fetch: recording };
  await admin.repos.create({ name: repo });
  alice = await server.actor('user:alice', repo, 'writer', opts);
  bob = await server.actor('user:bob', repo, 'reviewer', opts);
  carol = await server.actor('user:carol', repo, 'integrator', opts);
  ci = await server.actor('service:ci', repo, 'runner', opts);
});
afterAll(async () => {
  await server?.stop();
});

const attest = (v: { tree_id: string; check: string; policy_version: number }) =>
  signAttestation(
    {
      id: `att_${Buffer.from(crypto.getRandomValues(new Uint8Array(10))).toString('hex')}`,
      ...v,
      result: 'pass',
      environment_digest: 'sha256:tour',
      issuer: 'service:ci',
      issued_at_ms: Date.now(),
    },
    ciKey,
  );

describe('the tour', () => {
  let seedHead = '';

  it('session, repositories, roles and credentials', async () => {
    await admin.session.negotiate();
    // public, like negotiate: a server without identity providers offers the pasted credential only
    expect(await admin.session.authConfig()).toEqual({ mode: 'token' });
    expect((await admin.session.whoami()).identity).toBe('user:bootstrap-admin');
    expect((await admin.repos.describe({ repo })).name).toBe(repo);
    expect((await admin.repos.list({ prefix: 'tour/' })).repos).toHaveLength(1);
    expect((await admin.repos.configure({ repo, description: 'the tour' })).description).toBe('the tour');

    const binding = await admin.roles.grant({ subject: 'user:viewer', repo, role: 'reader' });
    expect(binding.role).toBe('reader');
    expect((await admin.roles.list({ repo })).bindings.some((b) => b.subject === 'user:viewer')).toBe(true);
    await admin.roles.revoke({ subject: 'user:viewer', repo, role: 'reader' });
    expect((await admin.roles.list({ subject: 'user:viewer' })).bindings).toHaveLength(0);

    const minted = await admin.credentials.mint({ subject: 'user:temp', ttl_secs: 60, description: 'tour' });
    expect(minted.token).toMatch(/^agt_/);
    expect((await admin.credentials.list({ subject: 'user:temp' })).credentials).toHaveLength(1);
    expect((await admin.credentials.revoke({ credential_id: minted.credential.id })).revoked).toBe(true);
  });

  it('policy', async () => {
    const doc = {
      trusted_issuers: [{ identity: 'service:ci', public_key: ciKey.publicKeyHex }],
      required_validations: [{ check: 'tests', issuers: ['service:ci'], run: { argv: ['true'] } }],
      approvals: { min: 1 },
    };
    const v = await admin.admin.policySet({ repo, stream: 'main', document: JSON.stringify(doc) });
    expect(v.version).toBe(1);
    expect(JSON.parse((await admin.admin.policyGet({ repo, stream: 'main' })).document)).toMatchObject({
      approvals: { min: 1 },
    });
    expect((await admin.admin.policyHistory({ repo, stream: 'main' })).versions).toHaveLength(1);
  });

  it('workspaces, proposals, reviews and the execution plane', async () => {
    expect((await alice.workspaces.create({ repo, name: 'w1', stream: 'main' })).name).toBe('w1');
    expect((await alice.workspaces.list({ repo })).workspaces.map((w) => w.name)).toContain('w1');
    const m1 = await alice.workspaces.modify({
      repo,
      name: 'w1',
      message: 'seed',
      changes: [putFile('a.txt', 'a\n'), putFile('b/c.txt', 'c\n')],
    });
    expect((await alice.workspaces.get({ repo, name: 'w1' })).head).toBe(m1.head);
    expect((await alice.workspaces.sync({ repo, name: 'w1' })).status).toBe('up_to_date');

    const p1 = await alice.proposals.create({ repo, stream: 'main', head: m1.head, title: 'seed' });
    expect((await alice.proposals.list({ repo })).proposals).toHaveLength(1);
    // move the proposal to a newer head
    const m2 = await alice.workspaces.modify({ repo, name: 'w1', message: 'more', changes: [putFile('d.txt', 'd\n')] });
    expect((await alice.proposals.update({ repo, id: p1.id, new_head: m2.head })).head).toBe(m2.head);
    expect((await alice.proposals.get({ repo, id: p1.id })).proposal.head).toBe(m2.head);
    await bob.proposals.review({ repo, id: p1.id, decision: 'approve' });

    // validation: queued, claimed, failed once, claimed again, completed with a signed attestation
    const queued = await alice.integration.validate({ repo, proposal: p1.id });
    expect(queued.jobs).toHaveLength(1);
    const first = (await ci.jobs.claim({ repo, worker: 'tour' })).job!;
    await ci.jobs.heartbeat({ repo, job_id: first.id, worker: 'tour' });
    await ci.jobs.fail({ repo, job_id: first.id, worker: 'tour', error: 'flaky', retry_after_ms: 1 });
    await new Promise((r) => setTimeout(r, 20));
    const second = (await ci.jobs.claim({ repo, worker: 'tour', wait_ms: 3000 })).job!;
    const tree = await ci.jobs.candidate({ repo, proposal: p1.id, expected_tree_id: second.validation!.tree_id });
    expect(tree.total_entries).toBe(3);
    const done = await ci.jobs.complete({
      repo,
      job_id: second.id,
      worker: 'tour',
      attestation: attest(second.validation!),
    });
    expect(done.state).toBe('succeeded');
    expect((await ci.jobs.get({ repo, job_id: second.id })).state).toBe('succeeded');
    expect((await ci.jobs.list({ repo })).jobs).toHaveLength(1);
    expect(
      (await carol.integration.attestations({ repo, tree_id: second.validation!.tree_id })).attestations,
    ).toHaveLength(1);

    // the queue lands it
    expect((await carol.integration.prepare({ repo, proposal: p1.id })).blockers).toHaveLength(0);
    const entry = await carol.queue.enqueue({ repo, proposal: p1.id });
    expect(entry.state).toBe('queued');
    expect((await carol.queue.list({ repo, stream: 'main' })).entries).toHaveLength(1);
    const ran = await carol.queue.run({ repo, stream: 'main' });
    // (queue states: queued, done, blocked, rejected, failed; outcome explains "done": "integrated")
    expect(ran.processed[0]).toMatchObject({ state: 'done', outcome: 'integrated' });
    seedHead = m2.head;

    // a second proposal is attested directly and integrated; a third is closed
    await alice.workspaces.create({ repo, name: 'w2', stream: 'main' });
    const m3 = await alice.workspaces.modify({ repo, name: 'w2', changes: [putFile('e.txt', 'e\n')] });
    const p2 = await alice.proposals.create({ repo, stream: 'main', head: m3.head, title: 'second' });
    await bob.proposals.review({ repo, id: p2.id, decision: 'comment', body: 'hm' });
    await bob.proposals.review({ repo, id: p2.id, decision: 'approve' });
    const prep = await carol.integration.prepare({ repo, proposal: p2.id });
    expect(
      (
        await ci.integration.attest({
          repo,
          attestation: attest({ tree_id: prep.candidate_tree, check: 'tests', policy_version: prep.policy_version }),
        })
      ).accepted,
    ).toBe(true);
    expect((await carol.integration.integrate({ repo, proposal: p2.id })).outcome.status).toBe('integrated');
    await alice.workspaces.create({ repo, name: 'w3', stream: 'main' });
    const m4 = await alice.workspaces.modify({ repo, name: 'w3', changes: [putFile('f.txt', 'f\n')] });
    const p3 = await alice.proposals.create({ repo, stream: 'main', head: m4.head, title: 'third' });
    expect((await alice.proposals.close({ repo, id: p3.id })).state).toBe('closed');
    await alice.workspaces.delete({ repo, name: 'w3' });
  });

  it('history, trees, streams and objects', async () => {
    const head = (await alice.streams.head({ repo, stream: 'main' })).head;
    expect((await alice.history.resolve({ repo, rev: 'main' })).changeset_id).toBe(head);
    expect((await alice.history.log({ repo, rev: 'main' })).changesets.length).toBeGreaterThan(1);
    expect((await alice.history.changeset({ repo, rev: 'main', include_ops: true })).id).toBe(head);
    expect((await alice.history.mergeBase({ repo, a: 'main', b: seedHead })).bases).toEqual([seedHead]);
    const anc = await alice.history.ancestors({ repo, want: [head], have: [seedHead] });
    expect(anc.changeset_ids).toEqual([head]);

    expect(
      (await alice.tree.search({ repo, rev: 'main', query: 'c', path_prefix: 'b' })).matches.map((m) => m.path),
    ).toEqual(['b/c.txt']);
    expect((await alice.tree.get({ repo, rev: 'main' })).total_entries).toBe(4);
    expect((await alice.tree.list({ repo, rev: 'main', path: 'b' })).entries.map((e) => e.name)).toEqual(['c.txt']);
    expect((await alice.tree.resolve({ repo, rev: 'main', path: 'a.txt' })).kind).toBe('file');
    expect((await alice.tree.diff({ repo, from: seedHead, to: 'main' })).files.map((f) => f.path)).toEqual(['e.txt']);
    expect(new TextDecoder().decode(await alice.tree.file({ repo, path: 'b/c.txt', rev: 'main' }))).toBe('c\n');

    expect((await alice.streams.list({ repo })).streams.map((s) => s.name)).toContain('main');
    await alice.streams.create({ repo, name: 'dev' });
    const cas = await admin.streams.cas({
      repo,
      stream: 'dev',
      expected_version: 0,
      expected_head: '',
      new_head: head,
      reason: 'tour',
    });
    expect(cas).toMatchObject({ name: 'dev', head, version: 1 });
    expect((await alice.streams.reflog({ repo, stream: 'dev' })).entries).toHaveLength(1);

    // objects: build a changeset client-side, upload it, ask about it, publish it
    const ns = (await alice.repos.describe({ repo })).namespace;
    const built = buildChangeset({
      namespace: ns,
      parents: [head],
      author: 'user:alice',
      timestampMs: 1_760_000_000_000,
      ops: [{ op: 'put', path: 'g.txt', content: 'g\n' }],
    });
    const everything = [...built.blobs, built.object];
    expect((await alice.objects.missing({ repo, object_ids: everything.map((o) => o.id) })).missing).toHaveLength(2);
    for (const o of everything) await alice.objects.put({ repo, object_id: o.id, data: o.envelope });
    expect((await alice.objects.missing({ repo, object_ids: everything.map((o) => o.id) })).missing).toHaveLength(0);
    expect(Array.from(await alice.objects.get({ repo, object_id: blobObject('g\n').id }))).toEqual(
      Array.from(blobObject('g\n').envelope),
    );
    expect((await alice.objects.verify({ repo, object_ids: [built.id] })).objects[0]?.status).toBe('ok');
    const pub = await alice.objects.publish({ repo, head: built.id }, { idempotencyKey: 'tour-publish' });
    expect(pub.state).toBe('published');
    expect((await alice.operations.get({ repo, idempotency_key: 'tour-publish' })).kind).toBe('publish');
  });

  it('line-level blame', async () => {
    const blame = await alice.history.blame({ repo, path: 'b/c.txt', rev: 'main' });
    expect(blame.lines.map((l) => [l.line, l.text, l.path])).toEqual([[1, 'c', 'b/c.txt']]);
    expect(blame.lines[0]!.changeset).toMatch(/^[0-9a-f]{64}$/);
  });

  it('agents, tasks, leases and provenance', async () => {
    const bot = await server.actor('agent:tour-bot', repo, 'writer', { fetch: recording });
    const key = generateAgentKey();
    await bot.agents.register({ repo, name: 'tour-bot', public_key: key.publicKeyHex });
    expect((await alice.agents.list({ repo })).agents.map((a) => a.name)).toContain('tour-bot');
    const task = await alice.tasks.create({ repo, title: 'tour task', scope: ['src/**'] });
    const dep = await alice.tasks.create({ repo, title: 'depends' });
    await alice.tasks.depend({ repo, task: dep.id, depends_on: task.id });
    expect((await alice.tasks.list({ repo })).tasks).toHaveLength(2);
    const run = await bot.agents.startRun({ repo, agent: 'tour-bot', task_id: task.id });
    expect((await alice.agents.listRuns({ repo, active_only: true })).runs).toHaveLength(1);
    const runner = bot.withRun(run.run_id);
    await runner.agents.heartbeat({ repo, run_id: run.run_id });
    const lease = await runner.leases.acquire({
      repo,
      scope: 'src/**',
      exclusive: true,
      ttl_ms: 30_000,
      task_id: task.id,
    });
    expect((await alice.leases.list({ repo, active_only: true })).leases).toHaveLength(1);
    await runner.leases.extend({ repo, lease_id: lease.lease.id, ttl_ms: 60_000 });
    await runner.leases.release({ repo, lease_id: lease.lease.id });
    expect((await alice.agents.predict({ repo, stream: 'main' })).overlaps).toEqual(expect.any(Array));

    const head = (await alice.streams.head({ repo, stream: 'main' })).head;
    const record = { changeset: head, agent: run.identity, run_id: run.run_id, model: 'm', created_at_ms: Date.now() };
    const entry = await runner.provenance.submit({ repo, record, signature: signProvenance(record, key.privateKey) });
    expect(entry.verified).toBe(true);
    expect((await alice.provenance.list({ repo, changeset: head })).entries).toHaveLength(1);
    await runner.tasks.finish({ repo, id: task.id, state: 'done' });
    await runner.agents.complete({ repo, run_id: run.run_id, status: 'completed' });
    expect((await alice.tasks.get({ repo, id: dep.id })).ready).toBe(true);
    // The operator's call: nothing has been silent for an hour, so a dry run finds nothing to end.
    const plan = await admin.agents.reapRuns({ repo, stale_after_ms: 3_600_000, dry_run: true });
    expect(plan).toMatchObject({ dry_run: true, runs: [], released_leases: [], revoked_credentials: 0 });
  });

  it('preflights and publishes Git migration with a stable upload key', async () => {
    const data = new Uint8Array(readFileSync(path.join(REPO_ROOT, 'sdk/fixtures/git-migration.bundle')));
    const options = { idempotencyKey: 'sdk-migration-tour', timeoutMs: 300_000 };
    const plan = await admin.migrations.prepare({ name: 'tour/migrated', data }, options);
    const replay = await admin.migrations.prepare({ name: 'tour/migrated', data }, options);
    expect(replay.id).toBe(plan.id);
    expect((await admin.migrations.status({ id: plan.id })).state).toBe('ready');
    expect((await admin.migrations.commit({ id: plan.id, accept_lossy: false })).state).toBe('imported');
    expect((await admin.migrations.commit({ id: plan.id, accept_lossy: false })).state).toBe('imported');
    expect(
      new TextDecoder().decode(await admin.tree.file({ repo: 'tour/migrated', path: 'hello.txt', rev: 'main' })),
    ).toBe('hello from migration\n');
  });

  it('events, audit and maintenance', async () => {
    expect((await alice.events.replay({ repo, after_seq: 0, limit: 5 })).events).toHaveLength(5);
    const got: string[] = [];
    for await (const ev of alice.events.subscribe({ repo, afterSeq: 0 })) {
      got.push(ev.topic);
      if (got.length === 3) break;
    }
    expect(got).toHaveLength(3);
    expect((await admin.admin.audit({ repo, limit: 5 })).entries).toHaveLength(5);
    expect((await admin.admin.auditVerify({ repo })).ok).toBe(true);
    const head = (await alice.streams.head({ repo, stream: 'main' })).head;
    await admin.admin.pin({ repo, name: 'release-1', rev: head, reason: 'tour' });
    expect((await admin.admin.pins({ repo })).pins.map((p) => p.name)).toEqual(['release-1']);
    await admin.admin.unpin({ repo, name: 'release-1' });
    expect((await admin.admin.fsck({ repo })).ok).toBe(true);
    expect((await admin.admin.gc({ repo, dry_run: true })).dry_run).toBe(true);
  });
});

describe('coverage', () => {
  it('every operation of the OpenAPI document has a namespaced method', () => {
    const spec = JSON.parse(readFileSync(path.join(REPO_ROOT, 'docs/protocol/openapi.json'), 'utf8')) as {
      paths: Record<string, Record<string, { operationId: string }>>;
    };
    const ids = Object.values(spec.paths)
      .flatMap((p) => Object.values(p).map((o) => o.operationId))
      .sort();
    const methods = Object.values(NAMESPACE_METHODS)
      .flatMap((m) => Object.values(m))
      .sort();
    expect(methods).toEqual(ids);
    expect(Object.keys(OPERATIONS).sort()).toEqual(ids);
    // ... and the client really exposes each one
    const client = new AgentGitClient({ baseUrl: 'http://127.0.0.1:1' }) as unknown as Record<
      string,
      Record<string, unknown>
    >;
    for (const [ns, table] of Object.entries(NAMESPACE_METHODS)) {
      for (const name of Object.keys(table)) expect(typeof client[ns]?.[name], `${ns}.${name}`).toBe('function');
    }
  });

  it('the tour reached every operation, and no route was missing on the server', () => {
    const reached = new Set(hits.map((h) => h.id));
    const missing = (Object.keys(OPERATIONS) as OperationId[]).filter((id) => !reached.has(id));
    expect(missing).toEqual([]);
    expect(hits.filter((h) => h.routeMissing)).toEqual([]);
  });
});
