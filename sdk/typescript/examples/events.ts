// Follow a repository's event feed: replay the past, then stay live. The connection resumes after the
// last event it delivered if it drops.
//
//   AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... npx tsx examples/events.ts

import { AgentGitClient, putFile } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
const repo = `demo/events-${Date.now()}`;
await client.repos.create({ name: repo });

// #region events
const controller = new AbortController();
const seen: string[] = [];

// afterSeq: 0 replays the whole history first; leave it out to start from "now".
// topics are prefixes: "proposal" matches proposal.create, "stream" matches stream.integrate.
const feed = client.subscribeEvents({
  repo,
  afterSeq: 0,
  topics: ['proposal', 'stream'],
  signal: controller.signal,
});

// Something happens in the repository while we listen ...
const work = (async () => {
  await client.workspaces.create({ repo, name: 'w', stream: 'main' });
  const change = await client.workspaces.modify({ repo, name: 'w', changes: [putFile('a.txt', 'a\n')] });
  const proposal = await client.proposals.create({ repo, stream: 'main', head: change.head, title: 'A' });
  await client.integration.integrate({ repo, proposal: proposal.id });
})();

// ... and each event arrives exactly once, in order, with a strictly increasing seq.
for await (const event of feed) {
  console.log(event.seq, event.topic, event.payload['target']);
  seen.push(event.topic);
  if (event.topic === 'stream.integrate') controller.abort(); // ends the loop quietly
}
await work;
// #endregion events

/** What the test checks. */
export const eventsResult = { seen };
