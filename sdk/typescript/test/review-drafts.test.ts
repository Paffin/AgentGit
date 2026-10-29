import { expect, it } from 'vitest';
import { AgentGitClient } from '../src/index.js';

const draft = {
  present: true,
  version: 1,
  head: 'reviewed',
  decision: 'comment',
  body: 'Private notes',
  updated_at_ms: 1,
};

it('sends private draft reads and guarded save/clear requests without an owner parameter', async () => {
  const calls: { url: string; method: string; body: unknown }[] = [];
  const client = new AgentGitClient({
    baseUrl: 'http://draft.test',
    token: 'reviewer',
    fetch: async (input, init) => {
      calls.push({
        url: typeof input === 'string' ? input : input instanceof URL ? input.href : input.url,
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
      });
      return Response.json(draft);
    },
  });
  await client.proposals.getDraft({ repo: 'team/app', id: 'proposal' });
  await client.proposals.saveDraft({
    repo: 'team/app',
    id: 'proposal',
    expected_version: 0,
    head: 'reviewed',
    decision: 'comment',
    body: 'Private notes',
  });
  await client.proposals.clearDraft({ repo: 'team/app', id: 'proposal', expected_version: 1 });
  expect(calls.map((call) => call.method)).toEqual(['GET', 'PUT', 'DELETE']);
  expect(
    calls.every((call) => new URL(call.url).pathname.endsWith('/v1/repos/team/app/proposals/proposal/review-draft')),
  ).toBe(true);
  expect(calls[1]!.body).toEqual({ expected_version: 0, head: 'reviewed', decision: 'comment', body: 'Private notes' });
  expect(calls[2]!.body).toBeUndefined();
  expect(new URL(calls[2]!.url).searchParams.get('expected_version')).toBe('1');
});

it('retries a lost private-draft save response with the same inspected version and content', async () => {
  const sent: string[] = [];
  const client = new AgentGitClient({
    baseUrl: 'http://draft.test',
    token: 'reviewer',
    sleep: async () => {},
    fetch: async (_input, init) => {
      if (typeof init?.body !== 'string') throw new Error('Expected JSON');
      sent.push(init.body);
      if (sent.length === 1) throw new TypeError('response lost after save');
      return Response.json(draft);
    },
  });
  await expect(
    client.proposals.saveDraft({
      repo: 'team/app',
      id: 'proposal',
      expected_version: 0,
      head: 'reviewed',
      decision: 'comment',
      body: 'Private notes',
    }),
  ).resolves.toEqual(draft);
  expect(sent).toHaveLength(2);
  expect(sent[0]).toBe(sent[1]);
});

it('retries clear using the same query guard, including version zero', async () => {
  const urls: string[] = [];
  const client = new AgentGitClient({
    baseUrl: 'http://draft.test',
    token: 'reviewer',
    sleep: async () => {},
    fetch: async (input, init) => {
      urls.push(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
      expect(init?.body).toBeUndefined();
      if (urls.length === 1) throw new TypeError('response lost after clear');
      return Response.json({ ...draft, present: false, body: '', head: '', decision: '', version: 2 });
    },
  });
  await expect(
    client.proposals.clearDraft({ repo: 'team/app', id: 'proposal', expected_version: 0 }),
  ).resolves.toMatchObject({ present: false, version: 2 });
  expect(urls).toHaveLength(2);
  expect(urls[0]).toBe(urls[1]);
  expect(new URL(urls[0]!).searchParams.get('expected_version')).toBe('0');
});
