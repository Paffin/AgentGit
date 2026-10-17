// Errors, retries, idempotency keys and deadlines.
//
//   AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... npx tsx examples/errors.ts

import { AgentGitClient } from '@agentgit/client';

const baseUrl = process.env['AGENTGIT_URL']!;
const token = process.env['AGENTGIT_TOKEN']!;
const repo = `demo/errors-${Date.now()}`;

// #region errors
import { AgentGitError } from '@agentgit/client';

const client = new AgentGitClient({
  baseUrl,
  token,
  requestTimeoutMs: 10_000, // per attempt; sent to the server as Request-Timeout
  retry: { maxAttempts: 4 }, // exponential backoff with jitter; honours Retry-After
});

try {
  await client.proposals.get({ repo: 'demo/does-not-exist', id: 'prop_1' });
} catch (error) {
  if (error instanceof AgentGitError) {
    console.log(error.code); //       "not_found": a stable, machine-readable code
    console.log(error.status); //     404
    console.log(error.retryable); //  false: the SDK did not retry it, and you should not
    console.log(error.requestId); //  the id to quote when asking an operator for the server log
  }
}

// Mutations that the server can deduplicate carry an Idempotency-Key that is generated for you and
// reused by every retry, so a lost response never creates the repository twice. Pass your own key
// to make a retry of the whole program safe too.
await client.repos.create({ name: repo }, { idempotencyKey: `create-${repo}` });
await client.repos.create({ name: repo }, { idempotencyKey: `create-${repo}` }); // returns the recorded result

// Reusing a key for a different request is an error, not a retry.
try {
  await client.repos.create({ name: `${repo}-other` }, { idempotencyKey: `create-${repo}` });
} catch (error) {
  console.log(error instanceof AgentGitError && error.code); // "idempotency_key_reused"
}

// A deadline can expire while the server is still working. The error says so: ask for the outcome
// before repeating anything that is not idempotent.
try {
  await client.streams.head(
    { repo, stream: 'main', wait_version_gt: 1000, wait_ms: 20_000 },
    { timeoutMs: 300, retry: false },
  );
} catch (error) {
  if (error instanceof AgentGitError && error.is('deadline_exceeded')) {
    console.log(error.outcomeUnknown); // true
  }
}
// #endregion errors

/** What the test checks. */
export const errorsResult = { repo };
