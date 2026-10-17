import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AgentGitClient, putFile } from '../src/index.js';
import { FaultyNetwork } from './harness/faults.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
let admin: AgentGitClient;
const net = new FaultyNetwork();

beforeAll(async () => {
  server = await startServer();
  admin = server.admin({ fetch: net.fetch });
});
afterAll(async () => {
  await server?.stop();
});

async function collect<T>(it: AsyncIterable<T>): Promise<T[]> {
  const out: T[] = [];
  for await (const x of it) out.push(x);
  return out;
}

describe('paginate', () => {
  it('follows next_page_token across repository listings', async () => {
    for (let i = 0; i < 23; i++) await admin.repos.create({ name: `pages/r${String(i).padStart(2, '0')}` });
    net.seen.length = 0;
    const repos = await collect(admin.paginate('repo.list', { prefix: 'pages/', page_size: 10 }));
    expect(repos.map((r) => r.name)).toEqual(
      Array.from({ length: 23 }, (_, i) => `pages/r${String(i).padStart(2, '0')}`),
    );
    // 23 items at 10 per page = 3 requests, each carrying the previous page's token.
    const listCalls = net.to('/v1/repos?');
    expect(listCalls).toHaveLength(3);
    expect(new URL(listCalls[0]!.url).searchParams.get('page_token')).toBe('');
    expect(new URL(listCalls[1]!.url).searchParams.get('page_token')).not.toBe('');
    expect(new Set(repos.map((r) => r.name)).size).toBe(23); // no duplicates
  });

  it('yields whole pages with pages() and stops early with maxItems', async () => {
    const pages = await collect(admin.pages('repo.list', { prefix: 'pages/', page_size: 10 }));
    expect(pages.map((p) => p.repos.length)).toEqual([10, 10, 3]);
    expect(pages.map((p) => p.next_page_token === '')).toEqual([false, false, true]);

    net.seen.length = 0;
    const few = await collect(admin.paginate('repo.list', { prefix: 'pages/', page_size: 10 }, { maxItems: 4 }));
    expect(few).toHaveLength(4);
    expect(net.to('/v1/repos?')).toHaveLength(1); // the second page was never requested
  });

  it('paginates history, trees, proposals and the candidate listing', async () => {
    const repo = 'pages/work';
    await admin.repos.create({ name: repo });
    const alice = await server.actor('user:alice', repo, 'writer', { fetch: net.fetch });
    await alice.workspaces.create({ repo, name: 'main-ws', stream: 'main' });
    let head = '';
    for (let i = 0; i < 12; i++) {
      head = (
        await alice.workspaces.modify({
          repo,
          name: 'main-ws',
          message: `change ${i}`,
          changes: [putFile(`f${String(i).padStart(2, '0')}.txt`, `v${i}\n`)],
        })
      ).head;
    }
    const p = await alice.proposals.create({ repo, stream: 'main', head, title: 'twelve commits' });
    const carol = await server.actor('user:carol', repo, 'integrator', { fetch: net.fetch });
    expect((await carol.integration.integrate({ repo, proposal: p.id })).outcome.status).toBe('integrated');

    // history: newest first, 5 per page
    const log = await collect(alice.paginate('history.log', { repo, rev: 'main', page_size: 5 }));
    expect(log.map((c) => c.message)).toEqual(Array.from({ length: 12 }, (_, i) => `change ${11 - i}`));
    // the manifest of the tree: 12 entries, 5 per page
    const entries = await collect(alice.paginate('tree.get', { repo, rev: 'main', page_size: 5 }));
    expect(entries).toHaveLength(12);
    expect(entries.map((e) => e.path)).toEqual([...entries.map((e) => e.path)].sort());
    // a directory listing
    const names = await collect(alice.paginate('tree.list', { repo, rev: 'main', page_size: 5 }));
    expect(names).toHaveLength(12);

    // proposals: open ones only, filtered by author
    for (let i = 0; i < 7; i++) {
      await alice.workspaces.create({ repo, name: `p${i}`, stream: 'main' });
      const w = await alice.workspaces.modify({ repo, name: `p${i}`, changes: [putFile(`extra${i}.txt`, 'x')] });
      await alice.proposals.create({ repo, stream: 'main', head: w.head, title: `extra ${i}` });
    }
    const open = await collect(
      alice.paginate('proposal.list', { repo, state: 'open', author: 'user:alice', page_size: 3 }),
    );
    expect(open).toHaveLength(7);
    expect(open.every((x) => x.state === 'open')).toBe(true);
    const all = await collect(alice.paginate('proposal.list', { repo, page_size: 3 }));
    expect(all).toHaveLength(8);

    // The worker view of a candidate tree is a paginated manifest too (12 files plus the proposal's own).
    const some = open[0]!;
    const candidate = await collect(alice.paginate('jobs.candidate', { repo, proposal: some.id, page_size: 5 }));
    expect(candidate).toHaveLength(13);
    await expect(
      alice.jobs.candidate({ repo, proposal: some.id, expected_tree_id: 'ab'.repeat(32) }),
    ).rejects.toMatchObject({ code: 'stale_candidate', status: 409 });
  });

  it('an unknown page token is an error from the server, passed through', async () => {
    await expect(admin.repos.list({ page_token: 'garbage' }, { retry: false })).rejects.toMatchObject({
      code: 'invalid_argument',
    });
  });

  it('does not loop when a server keeps returning the same token', async () => {
    let calls = 0;
    const stuck = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      fetch: async () => {
        calls++;
        return new Response(JSON.stringify({ repos: [{ name: `x/${calls}` }], next_page_token: 'same' }), {
          headers: { 'content-type': 'application/json' },
        });
      },
    });
    const got = await collect(stuck.paginate('repo.list', {}));
    expect(got).toHaveLength(2); // first page, then one more that repeats the token, then it stops
    expect(calls).toBe(2);
  });
});
