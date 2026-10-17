// sdk/api-policy.json lists message fields that the protocol makes optional but that the SDK types
// as always present, because ag-server fills them in. This fails if the server ever answers null.

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { putFile } from '../src/index.js';
import { REPO_ROOT, startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
afterAll(async () => {
  await server?.stop();
});

const policy = JSON.parse(readFileSync(path.join(REPO_ROOT, 'sdk/api-policy.json'), 'utf8')) as {
  alwaysPresent: { fields: string[] };
};

const seen = new Map<string, unknown>();
beforeAll(async () => {
  server = await startServer();
  const admin = server.admin();
  const repo = 'acme/contract';
  const created = await admin.repos.create({ name: repo });
  seen.set('Repo.quota', created.quota);
  const alice = await server.actor('user:alice', repo, 'writer');
  const carol = await server.actor('user:carol', repo, 'integrator');
  const who = await alice.session.whoami();
  seen.set('Principal.scope', who.scope);
  seen.set('NegotiateResponse.limits', (await admin.session.negotiate()).limits);
  const minted = await admin.credentials.mint({ subject: 'user:x', ttl_secs: 60 });
  seen.set('MintCredentialResponse.credential', minted.credential);
  seen.set('Credential.scope', minted.credential.scope);

  await alice.workspaces.create({ repo, name: 'w', stream: 'main' });
  const ws = await alice.workspaces.modify({ repo, name: 'w', changes: [putFile('a', 'a')] });
  const p = await alice.proposals.create({ repo, stream: 'main', head: ws.head, title: 't' });
  seen.set('ProposalDetail.proposal', (await alice.proposals.get({ repo, id: p.id })).proposal);
  seen.set('IntegrationResult.outcome', (await carol.integration.integrate({ repo, proposal: p.id })).outcome);
  const bot = await server.actor('agent:contract-bot', repo, 'writer');
  seen.set('AcquireLeaseResponse.lease', (await bot.leases.acquire({ repo, scope: 'x/**', ttl_ms: 10_000 })).lease);
});

describe('fields typed as present', () => {
  it('every field listed in sdk/api-policy.json is a non-null object on a live server', () => {
    for (const field of policy.alwaysPresent.fields) {
      expect(seen.has(field), `${field} is not checked by this test`).toBe(true);
      const value = seen.get(field);
      expect(value, field).not.toBeNull();
      expect(typeof value, field).toBe('object');
    }
  });
});
