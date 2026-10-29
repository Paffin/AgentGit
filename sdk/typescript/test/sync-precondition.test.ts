import { expect, it } from 'vitest';
import { AgentGitClient, type Conflict, type SyncWorkspaceResponse } from '../src/index.js';

it('sends the inspected stream head with a workspace resolution', async () => {
  let body: unknown;
  const client = new AgentGitClient({
    baseUrl: 'http://sync.test',
    token: 'test',
    fetch: async (_input, init) => {
      if (typeof init?.body !== 'string') throw new Error('Expected a JSON request body');
      body = JSON.parse(init.body);
      return Response.json({ status: 'merged', head: 'merged', stream_head: 'inspected', conflicts: [] });
    },
  });
  const result = await client.workspaces.sync({
    repo: 'team/app',
    name: 'work',
    expected_stream_head: 'inspected',
    resolutions: [
      { path: 'image.bin', op: 'put', blob_id: 'original-blob', file_type: 'regular', executable: true, content: '' },
    ],
  });
  expect(body).toEqual({
    expected_stream_head: 'inspected',
    resolutions: [
      { path: 'image.bin', op: 'put', blob_id: 'original-blob', file_type: 'regular', executable: true, content: '' },
    ],
  });
  expect(result.stream_head).toBe('inspected');
});

// Older servers lack the additive snapshot field.
const older: SyncWorkspaceResponse = { status: 'up_to_date', head: 'old', conflicts: [] };
void older;

it.each([undefined, '', 'inspected'])('only retries a lost sync response with a nonempty pin (%s)', async (pin) => {
  const sent: string[] = [];
  const client = new AgentGitClient({
    baseUrl: 'http://sync.test',
    token: 'test',
    sleep: async () => {},
    fetch: async (_input, init) => {
      if (typeof init?.body !== 'string') throw new Error('Expected JSON');
      sent.push(init.body);
      if (sent.length === 1) throw new TypeError('response lost after applying the merge');
      return Response.json({ status: 'up_to_date', head: 'merged', stream_head: 'inspected', conflicts: [] });
    },
  });
  const operation = client.workspaces.sync({ repo: 'team/app', name: 'work', expected_stream_head: pin });
  if (pin) {
    await expect(operation).resolves.toMatchObject({ head: 'merged' });
    expect(sent).toHaveLength(2);
    expect(sent[0]).toBe(sent[1]);
  } else {
    await expect(operation).rejects.toMatchObject({ attempts: 1, outcomeUnknown: true });
    expect(sent).toHaveLength(1);
  }
});

it('retains explicit unsafe-retry opt-in for an unpinned sync', async () => {
  let calls = 0;
  const client = new AgentGitClient({
    baseUrl: 'http://sync.test',
    token: 'test',
    sleep: async () => {},
    fetch: async () => {
      if (++calls === 1) throw new TypeError('response lost');
      return Response.json({ status: 'up_to_date', head: 'done', conflicts: [] });
    },
  });
  await expect(
    client.workspaces.sync({ repo: 'team/app', name: 'work' }, { retry: { retryUnsafe: true } }),
  ).resolves.toMatchObject({ head: 'done' });
  expect(calls).toBe(2);
});

it('preserves distinct source paths for a renamed conflict and accepts legacy conflicts', async () => {
  const conflict: Conflict = {
    path: 'renamed.txt',
    kind: 'content',
    message: 'both edited',
    origin: { base: 'original.txt', ours: 'renamed.txt', theirs: 'original.txt' },
  };
  const legacy: Conflict = { path: 'same.txt', kind: 'content', message: '' };
  const client = new AgentGitClient({
    baseUrl: 'http://sync.test',
    token: 'test',
    fetch: async () =>
      Response.json({ status: 'conflicted', head: 'workspace', stream_head: 'stream', conflicts: [conflict, legacy] }),
  });
  const result = await client.workspaces.sync({ repo: 'team/app', name: 'work' });
  expect(result.conflicts[0]?.origin).toEqual(conflict.origin);
  expect(result.conflicts[1]?.origin).toBeUndefined();
});
