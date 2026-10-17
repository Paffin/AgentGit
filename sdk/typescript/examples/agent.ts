// An agent run end to end: its own key, a run, work in a virtual workspace, a proposal,
// signed provenance, and a human who approves and integrates.
//
//   AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_ADMIN_TOKEN=... npx tsx examples/agent.ts

import { AgentGitClient } from '@agentgit/client';

const url = process.env['AGENTGIT_URL']!;
const admin = new AgentGitClient({ baseUrl: url, token: process.env['AGENTGIT_ADMIN_TOKEN']! });

// #region agent-setup
// An operator creates the repository, requires signed provenance from agents, and gives the agent a
// short-lived credential that is limited to this repository.
const repo = `demo/agent-${Date.now()}`;
await admin.repos.create({ name: repo });
await admin.admin.policySet({
  repo,
  stream: 'main',
  document: JSON.stringify({ require_provenance_for_agents: true, approvals: { min_for_agents: 1 } }),
});
await admin.roles.grant({ subject: 'agent:docs-bot', repo, role: 'writer' });
await admin.roles.grant({ subject: 'user:reviewer', repo, role: 'reviewer' });
await admin.roles.grant({ subject: 'user:maintainer', repo, role: 'integrator' });
const minted = await admin.credentials.mint({
  subject: 'agent:docs-bot',
  ttl_secs: 900,
  scope: { repos: [repo] },
});
// #endregion agent-setup

// #region agent
import { generateAgentKey, putFile, signProvenance } from '@agentgit/client';

const bot = new AgentGitClient({ baseUrl: url, token: minted.token });

// The agent registers the public half of its own key, then starts a run.
const key = generateAgentKey();
await bot.agents.register({ repo, name: 'docs-bot', public_key: key.publicKeyHex });
const run = await bot.agents.startRun({ repo, agent: 'docs-bot' });
console.log(run.identity); // agent:docs-bot#run_...

// Everything the agent does from here on is attributed to this run.
const work = bot.withRun(run.run_id);
await work.workspaces.create({ repo, name: 'docs', stream: 'main' });
const change = await work.workspaces.modify({
  repo,
  name: 'docs',
  message: 'document the build',
  changes: [putFile('docs/build.md', '# Building\n\nRun `make`.\n')],
});
const proposal = await work.proposals.create({ repo, stream: 'main', head: change.head, title: 'Document the build' });

// Signed provenance: who made this change, with what, and why. The server verifies the signature
// against the key registered for the agent.
const record = {
  changeset: change.head,
  agent: run.identity,
  run_id: run.run_id,
  model: 'example-model',
  rationale: 'the build steps were undocumented',
  created_at_ms: Date.now(),
};
const entry = await work.provenance.submit({ repo, record, signature: signProvenance(record, key.privateKey) });
console.log(entry.verified); // true

await work.agents.complete({ repo, run_id: run.run_id, status: 'completed', summary: 'documented the build' });
// #endregion agent

// #region agent-review
// Humans decide. The policy above holds the change until someone approves it, and only an
// integrator can land it.
const reviewer = await humanClient('user:reviewer');
const maintainer = await humanClient('user:maintainer');
const held = await maintainer.integration.integrate({ repo, proposal: proposal.id });
console.log(
  held.outcome.status,
  held.outcome.blockers.map((b) => b.code),
); // blocked [ 'insufficient_approvals' ]

await reviewer.proposals.review({ repo, id: proposal.id, decision: 'approve', body: 'looks right' });
const landed = await maintainer.integration.integrate({ repo, proposal: proposal.id });
console.log(landed.outcome.status); // integrated
// #endregion agent-review

async function humanClient(subject: string): Promise<AgentGitClient> {
  const { token } = await admin.credentials.mint({ subject, ttl_secs: 900 });
  return new AgentGitClient({ baseUrl: url, token });
}

/** What the test checks. */
export const agentResult = {
  repo,
  runIdentity: run.identity,
  verified: entry.verified,
  held: held.outcome.blockers.map((b) => b.code),
  landed: landed.outcome.status,
};
