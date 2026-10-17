// A complete change life cycle against a real server: a writer edits a virtual workspace, opens a
// proposal, a reviewer approves, an integrator integrates, and everything is read back.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { blobObject, buildChangeset, deleteFile, putFile, utf8Decode, type AgentGitClient } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
const repo = 'acme/web';
let admin: AgentGitClient;
let alice: AgentGitClient;
let bob: AgentGitClient;
let carol: AgentGitClient;

beforeAll(async () => {
  server = await startServer();
  admin = server.admin();
  await admin.repos.create({ name: repo, description: 'demo' });
  alice = await server.actor('user:alice', repo, 'writer');
  bob = await server.actor('user:bob', repo, 'reviewer');
  carol = await server.actor('user:carol', repo, 'integrator');
  await admin.admin.policySet({ repo, stream: 'main', document: JSON.stringify({ approvals: { min: 1 } }) });
});
afterAll(async () => {
  await server?.stop();
});

describe('writer -> reviewer -> integrator', () => {
  let proposalId = '';
  let head = '';

  it('a writer changes files in a virtual workspace and opens a proposal', async () => {
    const ws = await alice.workspaces.create({ repo, name: 'ws-1', stream: 'main' });
    expect(ws.owner).toBe('user:alice');
    const changed = await alice.workspaces.modify({
      repo,
      name: 'ws-1',
      message: 'initial import',
      changes: [
        putFile('README.md', '# acme\n'),
        putFile('src/lib.rs', 'fn main() {}\n'),
        putFile('bin/run.sh', '#!/bin/sh\n', { executable: true }),
      ],
    });
    head = changed.head;
    expect(head).toMatch(/^[0-9a-f]{64}$/);

    const proposal = await alice.proposals.create({
      repo,
      stream: 'main',
      head,
      title: 'Initial import',
      description: 'first files',
    });
    proposalId = proposal.id;
    expect(proposal).toMatchObject({ state: 'open', author: 'user:alice', head, stream: 'main' });
  });

  it('separation of duties: the author may neither review nor integrate', async () => {
    await expect(alice.proposals.review({ repo, id: proposalId, decision: 'approve' })).rejects.toMatchObject({
      code: 'permission_denied',
    });
    await expect(alice.integration.integrate({ repo, proposal: proposalId })).rejects.toMatchObject({
      code: 'permission_denied',
    });
  });

  it('an integration without approval is blocked, with a precise reason, and changes nothing', async () => {
    const result = await carol.integration.integrate({ repo, proposal: proposalId });
    expect(result.outcome.status).toBe('blocked');
    expect(result.outcome.blockers[0]?.code).toBe('insufficient_approvals');
    expect((await alice.streams.head({ repo, stream: 'main' })).head).toBe('');

    // prepare tells the same story without side effects
    const prep = await carol.integration.prepare({ repo, proposal: proposalId });
    expect(prep.strategy).toBe('fast_forward');
    expect(prep.changed_files.sort()).toEqual(['README.md', 'bin/run.sh', 'src/lib.rs']);
    expect(prep.blockers).toHaveLength(1);
  });

  it('a reviewer approves the head; the integrator lands it exactly once', async () => {
    const review = await bob.proposals.review({ repo, id: proposalId, decision: 'approve', body: 'lgtm' });
    expect(review).toMatchObject({ decision: 'approve', reviewer: 'user:bob', head });

    const detail = await alice.proposals.get({ repo, id: proposalId });
    expect(detail.reviews).toHaveLength(1);

    const done = await carol.integration.integrate(
      { repo, proposal: proposalId },
      { idempotencyKey: 'integrate-once' },
    );
    expect(done.outcome).toMatchObject({
      status: 'integrated',
      new_head: head,
      strategy: 'fast_forward',
      version: 1,
      changed_files: 3,
    });

    // The same key returns the recorded result instead of integrating again ...
    const again = await carol.integration.integrate(
      { repo, proposal: proposalId },
      { idempotencyKey: 'integrate-once' },
    );
    expect(again.operation_id).toBe(done.operation_id);
    // ... and the outcome can be asked for after a lost response.
    const op = await carol.operations.get({ repo, idempotency_key: 'integrate-once' });
    expect(op.integration?.outcome.status).toBe('integrated');
    expect((await alice.proposals.get({ repo, id: proposalId })).proposal.state).toBe('integrated');
  });

  it('reads everything back: head, files, directory, tree, log, diff, reflog and events', async () => {
    const stream = await alice.streams.head({ repo, stream: 'main' });
    expect(stream).toMatchObject({ head, version: 1 });

    const lib = await alice.tree.file({ repo, path: 'src/lib.rs', rev: 'main' });
    expect(lib).toBeInstanceOf(Uint8Array);
    expect(utf8Decode(lib)).toBe('fn main() {}\n');

    const list = await alice.tree.list({ repo, rev: 'main' });
    expect(list.entries.map((e) => e.name).sort()).toEqual(['README.md', 'bin', 'src']);

    const resolved = await alice.tree.resolve({ repo, rev: 'main', path: 'bin/run.sh' });
    expect(resolved.kind).toBe('file');
    expect(resolved.entry?.executable).toBe(true);

    const tree = await alice.tree.get({ repo, rev: 'main' });
    expect(tree.total_entries).toBe(3);
    expect(tree.entries.map((e) => e.path)).toEqual(['README.md', 'bin/run.sh', 'src/lib.rs']);

    const log = await alice.history.log({ repo, rev: 'main' });
    expect(log.changesets.map((c) => c.message)).toEqual(['initial import']);
    const cs = await alice.history.changeset({ repo, rev: 'main', include_ops: true });
    expect(cs.ops.map((o) => `${o.op}:${o.path}`)).toEqual(['put:README.md', 'put:bin/run.sh', 'put:src/lib.rs']);
    expect((await alice.history.resolve({ repo, rev: 'main' })).changeset_id).toBe(head);

    const diff = await alice.tree.diff({ repo, to: 'main' });
    expect(diff.files.map((f) => f.status)).toEqual(['added', 'added', 'added']);
    expect(diff.files.find((f) => f.path === 'README.md')?.patch).toContain('+# acme');

    const reflog = await alice.streams.reflog({ repo, stream: 'main' });
    expect(reflog.entries[0]).toMatchObject({ new_head: head, version: 1 });

    const events = await alice.events.replay({ repo, after_seq: 0, limit: 100 });
    const topics = events.events.map((e) => e.topic);
    expect(topics).toEqual(expect.arrayContaining(['workspace.modify', 'proposal.create', 'stream.integrate']));
    const seqs = events.events.map((e) => e.seq);
    expect([...seqs].sort((a, b) => a - b)).toEqual(seqs);
    expect((await admin.admin.auditVerify({ repo })).ok).toBe(true);
  });

  it('a later change that edits and deletes files fast-forwards with the right diff', async () => {
    await alice.workspaces.create({ repo, name: 'ws-2', stream: 'main' });
    const changed = await alice.workspaces.modify({
      repo,
      name: 'ws-2',
      message: 'edit and delete',
      changes: [putFile('README.md', '# acme\nmore\n'), deleteFile('src/lib.rs')],
      expected_head: head,
    });
    const p = await alice.proposals.create({ repo, stream: 'main', head: changed.head, title: 'Edit' });
    await bob.proposals.review({ repo, id: p.id, decision: 'approve' });
    const done = await carol.integration.integrate({ repo, proposal: p.id });
    expect(done.outcome.status).toBe('integrated');
    const diff = await alice.tree.diff({ repo, from: head, to: 'main' });
    expect(diff.files.map((f) => `${f.status}:${f.path}`).sort()).toEqual(['deleted:src/lib.rs', 'modified:README.md']);
    await expect(alice.tree.file({ repo, path: 'src/lib.rs', rev: 'main' })).rejects.toMatchObject({
      code: 'not_found',
    });
    // Workspace revisions are addressable as @name.
    expect(utf8Decode(await alice.tree.file({ repo, path: 'README.md', rev: '@ws-2' }))).toBe('# acme\nmore\n');
    await alice.workspaces.delete({ repo, name: 'ws-2' });
    await expect(alice.workspaces.get({ repo, name: 'ws-2' })).rejects.toMatchObject({ code: 'not_found' });
  });
});

describe('a client that builds the changeset itself', () => {
  it('encodes ChangesetV1 locally, uploads only what is missing, proposes and integrates it', async () => {
    const info = await alice.repos.describe({ repo });
    const base = (await alice.streams.head({ repo, stream: 'main' })).head;
    const built = buildChangeset({
      namespace: info.namespace,
      parents: [base],
      author: 'user:alice',
      timestampMs: 1_760_659_200_000,
      message: 'add docs (built by the client)',
      ops: [
        { op: 'put', path: 'docs/guide.md', content: '# guide\n' },
        { op: 'put', path: 'docs/copy.md', content: '# guide\n' }, // same bytes: one blob
      ],
    });
    expect(built.blobs).toHaveLength(1);

    const first = await alice.objects.upload({ repo, objects: [...built.blobs, built.object] });
    expect(first.stored).toHaveLength(2);
    const second = await alice.objects.upload({ repo, objects: [...built.blobs, built.object] });
    expect(second.stored).toHaveLength(0);
    expect(second.alreadyPresent).toHaveLength(2);

    // The server re-derives the id from the bytes: both must agree.
    const fetched = await alice.objects.get({ repo, object_id: built.id });
    expect(Array.from(fetched)).toEqual(Array.from(built.object.envelope));
    const verified = await alice.objects.verify({ repo, object_ids: [built.id, ...built.blobs.map((b) => b.id)] });
    expect(verified.objects.every((o) => o.status === 'ok')).toBe(true);

    const proposal = await alice.proposals.create({ repo, stream: 'main', head: built.id, title: 'Docs' });
    await bob.proposals.review({ repo, id: proposal.id, decision: 'approve' });
    const done = await carol.integration.integrate({ repo, proposal: proposal.id });
    expect(done.outcome).toMatchObject({ status: 'integrated', new_head: built.id });
    expect(utf8Decode(await alice.tree.file({ repo, path: 'docs/guide.md', rev: 'main' }))).toBe('# guide\n');
  });

  it('putBlob stores a blob under the id blobId() computes', async () => {
    const res = await alice.objects.putBlob({ repo, content: 'hello\n' });
    expect(res.id).toBe('6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545');
    const again = await alice.objects.putBlob({ repo, content: 'hello\n' });
    expect(again.stored).toBe(false);
  });

  it('the server refuses bytes that do not hash to the id they are uploaded under', async () => {
    const { envelope } = blobObject('some bytes');
    await expect(
      alice.objects.put({ repo, object_id: 'ab'.repeat(32), data: envelope }, { retry: false }),
    ).rejects.toMatchObject({ code: 'object_corrupt', status: 422 });
  });
});
