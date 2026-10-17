// An agent run with a signed provenance record, leases, tasks and conflict prediction - against a real server.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  generateAgentKey,
  type AgentGitClient,
  putFile,
  signProvenance,
  utf8Decode,
  type ProvenanceRecordInput,
} from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
const repo = 'acme/agents';
let admin: AgentGitClient;
let alice: AgentGitClient;
let bob: AgentGitClient;
let carol: AgentGitClient;

beforeAll(async () => {
  server = await startServer();
  admin = server.admin();
  await admin.repos.create({ name: repo });
  alice = await server.actor('user:alice', repo, 'writer');
  bob = await server.actor('user:bob', repo, 'reviewer');
  carol = await server.actor('user:carol', repo, 'integrator');
  // Seed the stream with a human change.
  await alice.workspaces.create({ repo, name: 'seed', stream: 'main' });
  const seed = await alice.workspaces.modify({
    repo,
    name: 'seed',
    changes: [putFile('src/lib.rs', 'fn a() {}\nfn b() {}\n')],
  });
  const p = await alice.proposals.create({ repo, stream: 'main', head: seed.head, title: 'seed' });
  expect((await carol.integration.integrate({ repo, proposal: p.id })).outcome.status).toBe('integrated');
});
afterAll(async () => {
  await server?.stop();
});

describe('an agent run that lands signed work under policy', () => {
  it('registers a key, starts a run, edits a workspace, and is let in by a signed provenance record', async () => {
    await admin.admin.policySet({
      repo,
      stream: 'main',
      document: JSON.stringify({ require_provenance_for_agents: true, approvals: { min: 0, min_for_agents: 1 } }),
    });

    const bot = await server.actor('agent:refactor-bot', repo, 'writer', { scope: { repos: [repo] } });
    const key = generateAgentKey();
    const agent = await bot.agents.register({ repo, name: 'refactor-bot', public_key: key.publicKeyHex });
    expect(agent.identity).toBe('agent:refactor-bot');
    // An agent may only register itself.
    await expect(bot.agents.register({ repo, name: 'other-bot', public_key: key.publicKeyHex })).rejects.toMatchObject({
      code: 'permission_denied',
    });

    const run = await bot.agents.startRun({ repo, agent: 'refactor-bot' });
    expect(run.identity).toBe(`agent:refactor-bot#${run.run_id}`);
    expect(run.status).toBe('running');

    // From here on every request is attributed to the run.
    const runner = bot.withRun(run.run_id);
    await runner.workspaces.create({ repo, name: 'rename', stream: 'main' });
    const changed = await runner.workspaces.modify({
      repo,
      name: 'rename',
      message: 'rename a to alpha',
      changes: [putFile('src/lib.rs', 'fn alpha() {}\nfn b() {}\n'), putFile('NOTES.md', 'notes\n')],
    });
    const proposal = await runner.proposals.create({
      repo,
      stream: 'main',
      head: changed.head,
      title: 'Rename a to alpha',
    });
    expect(proposal.author).toBe(run.identity);

    // An agent cannot integrate on its own; and even a human integrator is blocked without provenance.
    await expect(runner.integration.integrate({ repo, proposal: proposal.id })).rejects.toMatchObject({
      code: 'permission_denied',
    });
    await bob.proposals.review({ repo, id: proposal.id, decision: 'approve' });
    const blocked = await carol.integration.integrate({ repo, proposal: proposal.id });
    expect(blocked.outcome.status).toBe('blocked');
    expect(blocked.outcome.blockers.map((b) => b.code)).toContain('missing_provenance');

    const record: ProvenanceRecordInput = {
      changeset: changed.head,
      agent: run.identity,
      run_id: run.run_id,
      model: 'example-model',
      rationale: 'rename for clarity',
      parameters: { temperature: '0.2', max_tokens: '4096' },
      checks_run: ['lint'],
      created_at_ms: Date.now(),
    };

    // A signature over a different record is refused ...
    const tampered = await runner.provenance
      .submit(
        { repo, record: { ...record, rationale: 'something else' }, signature: signProvenance(record, key.privateKey) },
        { retry: false },
      )
      .catch((e: unknown) => e);
    expect(tampered).toMatchObject({ code: 'invalid_argument', status: 400 });
    // ... and so is one made with a key that was not registered for the agent.
    const forger = generateAgentKey();
    await expect(
      runner.provenance.submit(
        { repo, record, signature: signProvenance(record, forger.privateKey) },
        { retry: false },
      ),
    ).rejects.toMatchObject({ code: 'invalid_argument' });

    // The agent's own signature is accepted: the server verifies exactly the bytes the SDK signed.
    const entry = await runner.provenance.submit({ repo, record, signature: signProvenance(record, key.privateKey) });
    expect(entry.verified).toBe(true);
    expect(entry.public_key).toBe(key.publicKeyHex);

    const listed = await alice.provenance.list({ repo, changeset: changed.head });
    expect(listed.entries).toHaveLength(1);
    expect(listed.entries[0]).toMatchObject({
      verified: true,
      record: { model: 'example-model', rationale: 'rename for clarity' },
    });
    expect(listed.entries[0]?.record?.parameters).toEqual({ max_tokens: '4096', temperature: '0.2' });

    // Now the integrator can land it, and the run ends.
    const done = await carol.integration.integrate({ repo, proposal: proposal.id });
    expect(done.outcome.status).toBe('integrated');
    expect(utf8Decode(await alice.tree.file({ repo, path: 'src/lib.rs', rev: 'main' }))).toBe(
      'fn alpha() {}\nfn b() {}\n',
    );

    const hb = await runner.agents.heartbeat({ repo, run_id: run.run_id });
    expect(hb.server_time_ms).toBeGreaterThan(0);
    const finished = await runner.agents.complete({
      repo,
      run_id: run.run_id,
      status: 'completed',
      summary: 'renamed',
    });
    expect(finished.status).toBe('completed');
    expect((await alice.agents.listRuns({ repo, agent: 'refactor-bot' })).runs[0]?.status).toBe('completed');
    expect((await alice.agents.list({ repo })).agents.map((a) => a.name)).toContain('refactor-bot');
    // A finished run can no longer be used as an identity.
    await expect(runner.workspaces.create({ repo, name: 'late', stream: 'main' })).rejects.toMatchObject({
      code: 'failed_precondition',
    });
  });

  it('exercises every key encoding: unicode, escapes and key order survive the signature round trip', async () => {
    const bot = await server.actor('agent:unicode-bot', repo, 'writer');
    const key = generateAgentKey();
    await bot.agents.register({ repo, name: 'unicode-bot', public_key: key.publicKeyHex });
    const run = await bot.agents.startRun({ repo, agent: 'unicode-bot' });
    const runner = bot.withRun(run.run_id);
    await runner.workspaces.create({ repo, name: 'uni', stream: 'main' });
    const changed = await runner.workspaces.modify({ repo, name: 'uni', changes: [putFile('u.txt', 'x')] });
    const record: ProvenanceRecordInput = {
      changeset: changed.head,
      agent: run.identity,
      run_id: run.run_id,
      rationale: 'quote " backslash \\ newline \n snowman ☃ crab \u{1f980} Привет',
      inputs: ['line\r\nbreak', '\u2028'],
      parameters: { '\u{10000}': 'astral', '\uffff': 'bmp', z: '1', é: '2', 'a-first': '3' },
      created_at_ms: 1_760_659_200_000,
    };
    const entry = await runner.provenance.submit({ repo, record, signature: signProvenance(record, key.privateKey) });
    expect(entry.verified).toBe(true);
    await runner.agents.complete({ repo, run_id: run.run_id, status: 'completed' });
  });
});

describe('leases and tasks', () => {
  it('soft leases coexist, exclusive leases refuse each other, fencing tokens only increase', async () => {
    const a1 = await server.actor('agent:worker-a', repo, 'writer');
    const a2 = await server.actor('agent:worker-b', repo, 'writer');
    const l1 = await a1.leases.acquire({ repo, scope: 'docs/**', exclusive: true, ttl_ms: 60_000 });
    expect(l1.lease.exclusive).toBe(true);
    const conflict = await a2.leases
      .acquire({ repo, scope: 'docs/api/**', exclusive: true, ttl_ms: 60_000 })
      .catch((e: unknown) => e);
    expect(conflict).toMatchObject({ code: 'lease_conflict', status: 409, retryable: false });

    const soft = await a2.leases.acquire({ repo, scope: 'docs/api/**', ttl_ms: 60_000 });
    expect(soft.overlapping.map((l) => l.id)).toEqual([l1.lease.id]);

    // Only the holder releases or extends.
    await expect(a2.leases.release({ repo, lease_id: l1.lease.id })).rejects.toMatchObject({
      code: 'failed_precondition',
    });
    const extended = await a1.leases.extend({ repo, lease_id: l1.lease.id, ttl_ms: 120_000 });
    expect(extended.fencing_token).toBe(l1.lease.fencing_token);
    expect(extended.expires_at_ms).toBeGreaterThan(l1.lease.expires_at_ms);
    expect((await a1.leases.list({ repo, active_only: true })).leases.length).toBeGreaterThanOrEqual(2);
    await a1.leases.release({ repo, lease_id: l1.lease.id });

    const l2 = await a2.leases.acquire({ repo, scope: 'docs/**', exclusive: true, ttl_ms: 60_000 });
    expect(l2.lease.fencing_token).toBeGreaterThan(l1.lease.fencing_token);
  });

  it('tasks form a dependency graph; cycles are refused; readiness follows completion', async () => {
    const extract = await alice.tasks.create({ repo, title: 'extract module', scope: ['src/**'] });
    const use = await alice.tasks.create({ repo, title: 'use module', depends_on: [extract.id] });
    expect(use.ready).toBe(false);
    const ready = await alice.tasks.list({ repo, ready_only: true });
    expect(ready.tasks.map((t) => t.id)).toContain(extract.id);
    expect(ready.tasks.map((t) => t.id)).not.toContain(use.id);

    await expect(alice.tasks.depend({ repo, task: extract.id, depends_on: use.id })).rejects.toMatchObject({
      code: 'failed_precondition',
    });
    await alice.tasks.finish({ repo, id: extract.id, state: 'done' });
    expect((await alice.tasks.get({ repo, id: use.id })).ready).toBe(true);

    const third = await alice.tasks.create({ repo, title: 'third' });
    const linked = await alice.tasks.depend({ repo, task: third.id, depends_on: use.id });
    expect(linked.depends_on).toEqual([use.id]);
  });

  it('predicts overlapping work on a stream', async () => {
    const prediction = await alice.agents.predict({ repo, stream: 'main' });
    expect(Array.isArray(prediction.overlaps)).toBe(true);
  });
});
