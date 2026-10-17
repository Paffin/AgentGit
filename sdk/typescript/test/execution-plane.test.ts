// Required checks: a trusted worker claims a validation job, signs an attestation for the exact
// candidate tree and completes the job; only then can the change land.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { generateAgentKey, putFile, signAttestation, type AgentGitClient, type AgentKey } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
let admin: AgentGitClient;
let alice: AgentGitClient;
let carol: AgentGitClient;
let ci: AgentGitClient;
const repo = 'acme/ci';
const ciKey: AgentKey = generateAgentKey();

beforeAll(async () => {
  server = await startServer();
  admin = server.admin();
  await admin.repos.create({ name: repo });
  alice = await server.actor('user:alice', repo, 'writer');
  carol = await server.actor('user:carol', repo, 'integrator');
  ci = await server.actor('service:ci', repo, 'runner');
  await admin.admin.policySet({
    repo,
    stream: 'main',
    document: JSON.stringify({
      trusted_issuers: [{ identity: 'service:ci', public_key: ciKey.publicKeyHex }],
      required_validations: [
        { check: 'tests', issuers: ['service:ci'], run: { argv: ['cargo', 'test'], timeout_secs: 600 } },
      ],
    }),
  });
});
afterAll(async () => {
  await server?.stop();
});

async function propose(name: string, file: string): Promise<{ id: string; head: string }> {
  await alice.workspaces.create({ repo, name, stream: 'main' });
  const ws = await alice.workspaces.modify({ repo, name, changes: [putFile(file, `${name}\n`)] });
  const p = await alice.proposals.create({ repo, stream: 'main', head: ws.head, title: name });
  return { id: p.id, head: ws.head };
}

const attestation = (
  job: { validation?: { tree_id: string; check: string; policy_version: number } | null },
  result: 'pass' | 'fail',
) => {
  const v = job.validation!;
  return signAttestation(
    {
      id: `att_${Buffer.from(crypto.getRandomValues(new Uint8Array(10))).toString('hex')}`,
      tree_id: v.tree_id,
      check: v.check,
      result,
      policy_version: v.policy_version,
      environment_digest: 'sha256:sdk-test',
      issuer: 'service:ci',
      issued_at_ms: Date.now(),
    },
    ciKey,
  );
};

describe('a validation job', () => {
  it('is queued, claimed, checked against the candidate tree, attested and completed - then the change lands', async () => {
    const p = await propose('feature-1', 'one.txt');
    const blocked = await carol.integration.integrate({ repo, proposal: p.id });
    expect(blocked.outcome.status).toBe('blocked');
    expect(blocked.outcome.blockers.map((b) => b.code)).toEqual(['missing_validation']);

    // Prepare can queue the jobs for the checks that have a runner configuration.
    const prep = await alice.integration.prepare({ repo, proposal: p.id, enqueue_validations: true });
    expect(prep.validations).toMatchObject([{ check: 'tests', state: 'pending' }]);
    expect(prep.jobs).toHaveLength(1);
    // Asking again does not queue a second job for the same candidate.
    const again = await alice.integration.validate({ repo, proposal: p.id });
    expect(again.jobs.map((j) => j.id)).toEqual(prep.jobs.map((j) => j.id));

    // Nothing for a caller who is not a runner.
    await expect(alice.jobs.claim({ repo, worker: 'nope' })).rejects.toMatchObject({ code: 'permission_denied' });

    const claimed = await ci.jobs.claim({ repo, worker: 'worker-1', kinds: ['validate'], lease_ms: 60_000 });
    const job = claimed.job!;
    expect(job).toMatchObject({ state: 'claimed', kind: 'validate', attempts: 1 });
    expect(job.claimed_by).toBe('service:ci|worker-1');
    expect(job.validation).toMatchObject({ proposal: p.id, check: 'tests', run: { argv: ['cargo', 'test'] } });

    // The worker checks out exactly the tree it was asked to check ...
    const files = [];
    for await (const e of ci.paginate('jobs.candidate', {
      repo,
      proposal: p.id,
      expected_tree_id: job.validation!.tree_id,
    })) {
      files.push(e.path);
    }
    expect(files).toEqual(['one.txt']);
    // ... and is told if the candidate changed in the meantime.
    await expect(ci.jobs.candidate({ repo, proposal: p.id, expected_tree_id: '0'.repeat(64) })).rejects.toMatchObject({
      code: 'stale_candidate',
    });

    const beat = await ci.jobs.heartbeat({ repo, job_id: job.id, worker: 'worker-1', lease_ms: 120_000 });
    expect(beat.claimed_until_ms).toBeGreaterThan(job.claimed_until_ms);
    expect((await ci.jobs.get({ repo, job_id: job.id })).state).toBe('claimed');

    // Another worker cannot finish this job; an attestation for another tree is refused.
    const other = await server.actor('service:ci', repo, 'runner');
    await expect(
      other.jobs.complete({ repo, job_id: job.id, worker: 'worker-2', attestation: attestation(job, 'pass') }),
    ).rejects.toMatchObject({ code: 'failed_precondition' });
    const wrong = { ...attestation(job, 'pass'), check: 'lint' };
    await expect(
      ci.jobs.complete({ repo, job_id: job.id, worker: 'worker-1', attestation: wrong }),
    ).rejects.toMatchObject({ code: 'invalid_argument' });

    const att = attestation(job, 'pass');
    const done = await ci.jobs.complete({
      repo,
      job_id: job.id,
      worker: 'worker-1',
      attestation: att,
      result: { exit_code: 0 },
    });
    expect(done.state).toBe('succeeded');
    expect(done.result).toMatchObject({ attestation: att.id, outcome: 'pass', exit_code: 0 });

    const listed = await carol.integration.attestations({ repo, tree_id: job.validation!.tree_id });
    expect(listed.attestations.map((a) => a.id)).toEqual([att.id]);

    const landed = await carol.integration.integrate({ repo, proposal: p.id });
    expect(landed.outcome.status).toBe('integrated');
    expect((await alice.jobs.list({ repo, state: 'succeeded' })).jobs.map((j) => j.id)).toContain(job.id);
  });

  it('a failing check denies the change; a failed job can be retried', async () => {
    const p = await propose('feature-2', 'two.txt');
    const { jobs } = await alice.integration.validate({ repo, proposal: p.id });
    const claimed = (await ci.jobs.claim({ repo, worker: 'worker-1' })).job!;
    expect(claimed.id).toBe(jobs[0]!.id);
    const fail = await ci.jobs.fail({
      repo,
      job_id: claimed.id,
      worker: 'worker-1',
      error: 'runner crashed',
      retry_after_ms: 1,
    });
    expect(fail.state).toBe('queued');
    await new Promise((r) => setTimeout(r, 20));
    const second = (await ci.jobs.claim({ repo, worker: 'worker-1', wait_ms: 2000 })).job!;
    expect(second).toMatchObject({ id: claimed.id, attempts: 2 });

    await ci.jobs.complete({ repo, job_id: second.id, worker: 'worker-1', attestation: attestation(second, 'fail') });
    const denied = await carol.integration.integrate({ repo, proposal: p.id });
    expect(denied.outcome.status).toBe('denied');
    expect(denied.outcome.blockers.map((b) => b.code)).toContain('failed_validation');
  });

  it('claiming with nothing queued answers null, and a long-poll returns when work arrives', async () => {
    expect((await ci.jobs.claim({ repo, worker: 'idle' })).job).toBeNull();
    const started = Date.now();
    const waiting = ci.jobs.claim({ repo, worker: 'poller', wait_ms: 10_000 });
    await new Promise((r) => setTimeout(r, 200));
    const p = await propose('feature-3', 'three.txt');
    await alice.integration.validate({ repo, proposal: p.id });
    const got = (await waiting).job!;
    expect(got.validation?.proposal).toBe(p.id);
    expect(Date.now() - started).toBeLessThan(8000);
    await ci.jobs.complete({ repo, job_id: got.id, worker: 'poller', stale: true });
    expect((await ci.jobs.get({ repo, job_id: got.id })).state).toBe('succeeded');
  });

  it('a trusted attestation can also be submitted directly, and an untrusted one is refused', async () => {
    const p = await propose('feature-4', 'four.txt');
    const prep = await carol.integration.prepare({ repo, proposal: p.id });
    const v = { tree_id: prep.candidate_tree, check: 'tests', policy_version: prep.policy_version };
    const good = attestation({ validation: v }, 'pass');
    expect((await ci.integration.attest({ repo, attestation: good })).accepted).toBe(true);
    expect((await carol.integration.integrate({ repo, proposal: p.id })).outcome.status).toBe('integrated');

    // A signature that does not match the content is refused outright ...
    const tampered = { ...attestation({ validation: v }, 'pass'), check: 'lint' };
    await expect(ci.integration.attest({ repo, attestation: tampered }, { retry: false })).rejects.toMatchObject({
      code: 'invalid_argument',
    });

    // ... and a well-formed one from a key the policy does not trust is stored but counts for nothing.
    const q = await propose('feature-5', 'five.txt');
    const prep2 = await carol.integration.prepare({ repo, proposal: q.id });
    const stranger = signAttestation(
      {
        id: `att_${'0'.repeat(20)}`,
        tree_id: prep2.candidate_tree,
        check: 'tests',
        result: 'pass',
        policy_version: prep2.policy_version,
        environment_digest: 'sha256:sdk-test',
        issuer: 'service:ci',
        issued_at_ms: Date.now(),
      },
      generateAgentKey(),
    );
    await ci.integration.attest({ repo, attestation: stranger });
    const still = await carol.integration.integrate({ repo, proposal: q.id });
    expect(still.outcome.status).toBe('blocked');
    expect(still.outcome.blockers.map((b) => b.code)).toEqual(['missing_validation']);
  });
});
