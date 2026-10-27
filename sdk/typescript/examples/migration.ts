// Preflight a Git bundle; set AGENTGIT_PUBLISH=1 only after reviewing the report.

// #region migration
import { readFile } from 'node:fs/promises';
import { AgentGitClient } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
const repo = process.env['AGENTGIT_REPO']!; // a new owner/name
const plan = await client.migrations.prepare(
  { name: repo, data: new Uint8Array(await readFile(process.env['AGENTGIT_BUNDLE'] ?? 'repository.bundle')) },
  { timeoutMs: 300_000 },
);
console.log(plan.id, plan.report);
// Review the report before opting into publication. Content omissions remain blocked.
if (process.env['AGENTGIT_PUBLISH'] === '1') {
  await client.migrations.commit({ id: plan.id, accept_lossy: false });
}
// #endregion migration

export const migrationResult = await client.migrations.status({ id: plan.id });
