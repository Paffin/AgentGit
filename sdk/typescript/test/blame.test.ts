// history.blame against a real server: which changeset introduced each line, line ranges, typed errors.
// The route exists in servers built from the "line-level blame" commit or later; against an older
// binary (which answers `no such route`) the suite is skipped.

import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { isAgentGitError, putFile, type AgentGitClient } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

const repo = 'blame/lines';
let server: RunningServer;
let alice: AgentGitClient;
let supported = true;
let first = '';
let second = '';

beforeAll(async () => {
  server = await startServer();
  const admin = server.admin();
  await admin.repos.create({ name: repo });
  alice = await server.actor('user:alice', repo, 'writer');

  // Two changesets land on main directly (stream.cas needs the admin role; no proposal is needed for a test).
  const base = await admin.streams.head({ repo, stream: 'main' });
  await alice.workspaces.create({ repo, name: 'w', stream: 'main' });
  const m1 = await alice.workspaces.modify({
    repo,
    name: 'w',
    message: 'first',
    changes: [putFile('x.txt', 'one\ntwo\nthree\n')],
  });
  await alice.objects.publish({ repo, head: m1.head });
  const c1 = await admin.streams.cas({
    repo,
    stream: 'main',
    expected_version: base.version,
    expected_head: base.head,
    new_head: m1.head,
    reason: 'blame test',
  });
  const m2 = await alice.workspaces.modify({
    repo,
    name: 'w',
    message: 'second',
    changes: [putFile('x.txt', 'one\nTWO\nthree\n')],
  });
  await alice.objects.publish({ repo, head: m2.head });
  await admin.streams.cas({
    repo,
    stream: 'main',
    expected_version: c1.version,
    expected_head: m1.head,
    new_head: m2.head,
    reason: 'blame test',
  });
  first = m1.head;
  second = m2.head;
  try {
    await alice.history.blame({ repo, path: 'x.txt' });
  } catch (e) {
    if (!isAgentGitError(e) || !e.message.includes('no such route')) throw e;
    supported = false;
  }
});
afterAll(async () => {
  await server?.stop();
});

describe('history.blame', () => {
  it('attributes every line to the changeset that last wrote it', async (ctx) => {
    if (!supported) ctx.skip();
    const blame = await alice.history.blame({ repo, path: 'x.txt', rev: 'main' });
    expect(blame.rev).toBe(second);
    expect(blame.truncated).toBe(false);
    expect(blame.lines.map((l) => [l.line, l.text, l.changeset, l.summary])).toEqual([
      [1, 'one', first, 'first'],
      [2, 'TWO', second, 'second'],
      [3, 'three', first, 'first'],
    ]);
    expect(blame.lines.every((l) => l.path === 'x.txt' && l.author === 'user:alice')).toBe(true);
  });

  it('honours first_line / last_line and a revision other than the tip', async (ctx) => {
    if (!supported) ctx.skip();
    const range = await alice.history.blame({ repo, path: 'x.txt', first_line: 2, last_line: 3 });
    expect(range.lines.map((l) => l.line)).toEqual([2, 3]);
    const before = await alice.history.blame({ repo, path: 'x.txt', rev: first });
    expect(before.lines.map((l) => l.text)).toEqual(['one', 'two', 'three']);
    expect(new Set(before.lines.map((l) => l.changeset))).toEqual(new Set([first]));
  });

  it('reports a missing file as a typed error', async (ctx) => {
    if (!supported) ctx.skip();
    const err = await alice.history.blame({ repo, path: 'nope.txt' }).catch((e: unknown) => e);
    expect(isAgentGitError(err)).toBe(true);
    expect(isAgentGitError(err) && err.retryable).toBe(false);
    expect(isAgentGitError(err) && err.status).toBeGreaterThanOrEqual(400);
  });
});
