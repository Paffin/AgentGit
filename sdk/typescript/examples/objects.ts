// Build a changeset on the client, upload only the objects the server lacks, and propose it.
//
//   AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... npx tsx examples/objects.ts

import { AgentGitClient } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
const repo = `demo/objects-${Date.now()}`;
await client.repos.create({ name: repo });

// #region objects
import { buildChangeset } from '@agentgit/client';

const info = await client.repos.describe({ repo }); // the namespace is part of every changeset hash
const me = await client.session.whoami();
const parent = (await client.streams.head({ repo, stream: 'main' })).head; // "" while the stream is empty

const built = buildChangeset({
  namespace: info.namespace,
  parents: parent ? [parent] : [],
  author: me.identity,
  timestampMs: Date.now(),
  message: 'add docs',
  ops: [
    { op: 'put', path: 'docs/guide.md', content: '# guide\n' },
    { op: 'put', path: 'docs/copy.md', content: '# guide\n' }, // same bytes: stored once
  ],
});

// Blobs first, then the changeset that names them. Objects the server already has are skipped.
const upload = await client.objects.upload({ repo, objects: [...built.blobs, built.object] });
console.log(upload.stored.length, upload.alreadyPresent.length); // 2 0

// The server derives the same id from the bytes, so the id can be proposed directly.
const proposal = await client.proposals.create({ repo, stream: 'main', head: built.id, title: 'Add docs' });
const landed = await client.integration.integrate({ repo, proposal: proposal.id });
console.log(landed.outcome.status); // integrated
// #endregion objects

/** What the test checks. */
export const objectsResult = { repo, id: built.id, newHead: landed.outcome.new_head, stored: upload.stored.length };
