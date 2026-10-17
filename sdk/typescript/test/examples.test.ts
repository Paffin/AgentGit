// The programs under examples/ are what the READMEs and docs/guides/sdks.md quote. They run here,
// against a real server, so that they cannot rot.

import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
beforeAll(async () => {
  server = await startServer();
  process.env['AGENTGIT_URL'] = server.baseUrl;
  process.env['AGENTGIT_TOKEN'] = server.bootstrapToken;
  process.env['AGENTGIT_ADMIN_TOKEN'] = server.bootstrapToken;
});
afterAll(async () => {
  await server?.stop();
});

/** Imports an example (which runs it) and returns what it printed and exported. */
async function run<T>(load: () => Promise<T>): Promise<{ module: T; printed: string[] }> {
  const printed: string[] = [];
  const spy = vi.spyOn(console, 'log').mockImplementation((...args: unknown[]) => {
    printed.push(args.map((a) => (typeof a === 'string' ? a : JSON.stringify(a))).join(' '));
  });
  try {
    return { module: await load(), printed };
  } finally {
    spy.mockRestore();
  }
}

describe('examples', () => {
  it('quickstart.ts', async () => {
    const { module, printed } = await run(() => import('../examples/quickstart.js'));
    expect(module.quickstartResult).toMatchObject({ status: 'integrated', readme: '# hello\n' });
    expect(printed[0]).toBe('integrated');
    expect(printed.at(-1)).toMatch(/^[0-9a-f]{12} add a greeting$/);
  });

  it('agent.ts', async () => {
    const { module, printed } = await run(() => import('../examples/agent.js'));
    expect(module.agentResult.runIdentity).toMatch(/^agent:docs-bot#run_/);
    expect(module.agentResult).toMatchObject({
      verified: true,
      held: ['insufficient_approvals'],
      landed: 'integrated',
    });
    expect(printed).toContain('true');
    expect(printed).toContain('integrated');
  });

  it('events.ts', async () => {
    const { module } = await run(() => import('../examples/events.js'));
    expect(module.eventsResult.seen).toEqual(['proposal.create', 'stream.integrate']);
  });

  it('errors.ts', async () => {
    const { printed } = await run(() => import('../examples/errors.js'));
    expect(printed.slice(0, 4)).toEqual(['not_found', '404', 'false', expect.stringMatching(/^sdk-/)]);
    expect(printed).toContain('idempotency_key_reused');
    expect(printed.at(-1)).toBe('true'); // outcomeUnknown after the deadline
  });

  it('objects.ts', async () => {
    const { module, printed } = await run(() => import('../examples/objects.js'));
    expect(module.objectsResult).toMatchObject({ stored: 2 });
    expect(module.objectsResult.newHead).toBe(module.objectsResult.id);
    expect(printed).toEqual(['2 0', 'integrated']);
  });

  it('signing.ts', async () => {
    const { module, printed } = await run(() => import('../examples/signing.js'));
    expect(module.signingResult).toMatchObject({
      blob: '6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545',
      valid: true,
      tampered: false,
    });
    expect(printed[0]).toBe('6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545');
  });
});
