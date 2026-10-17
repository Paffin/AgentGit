// A repository, a change made without a checkout, a proposal, an integration, and the history.
//
//   AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... npx tsx examples/quickstart.ts

// #region quickstart
import { AgentGitClient, putFile, utf8Decode } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
await client.negotiate({ clientName: 'quickstart' }); // checks that the server speaks agwp.v1

const repo = `demo/hello-${Date.now()}`;
await client.repos.create({ name: repo });

// Change files on the server, in a virtual workspace: no checkout needed.
await client.workspaces.create({ repo, name: 'hello', stream: 'main' });
const change = await client.workspaces.modify({
  repo,
  name: 'hello',
  message: 'add a greeting',
  changes: [putFile('README.md', '# hello\n'), putFile('src/main.rs', 'fn main() {}\n')],
});

// Propose it, and land it on the stream "main".
const proposal = await client.proposals.create({ repo, stream: 'main', head: change.head, title: 'Add a greeting' });
const result = await client.integration.integrate({ repo, proposal: proposal.id });
console.log(result.outcome.status); // "integrated"

// Read it back.
const readme = await client.tree.file({ repo, path: 'README.md', rev: 'main' });
console.log(utf8Decode(readme)); // "# hello"
for await (const changeset of client.paginate('history.log', { repo, rev: 'main' })) {
  console.log(changeset.id.slice(0, 12), changeset.message);
}
// #endregion quickstart

/** What the test checks. */
export const quickstartResult = { repo, status: result.outcome.status, readme: utf8Decode(readme) };
