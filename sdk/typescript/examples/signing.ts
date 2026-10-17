// Content ids, provenance signatures and client-side changesets need no server at all.
//
//   npx tsx examples/signing.ts

// #region signing
import { blobId, buildChangeset, generateAgentKey, signProvenance, verifyProvenance } from '@agentgit/client';

// The content id of a file: SHA-256 over "agentgit.object\0" || kind || version || length || bytes.
console.log(blobId('hello\n')); // 6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545

// Agents sign provenance records with their own Ed25519 key. The signature covers the record as
// compact JSON with sorted keys, under the domain "provenance.v1" - exactly what the server verifies.
const key = generateAgentKey();
const record = {
  changeset: 'ab'.repeat(32),
  agent: 'agent:docs-bot#run_0123456789abcdef0123',
  run_id: 'run_0123456789abcdef0123',
  model: 'example-model',
  created_at_ms: 1_760_659_200_000,
};
const signature = signProvenance(record, key.privateKey);
console.log(verifyProvenance(record, signature, key.publicKey)); // true
console.log(verifyProvenance({ ...record, model: 'other' }, signature, key.publicKey)); // false

// A client that has no use for virtual workspaces can encode ChangesetV1 itself. The id is the same
// one the server derives from the uploaded bytes.
const changeset = buildChangeset({
  namespace: 'demo/hello',
  author: 'user:alice',
  timestampMs: 1_760_659_200_000,
  message: 'add a greeting',
  ops: [{ op: 'put', path: 'README.md', content: '# hello\n' }],
});
console.log(changeset.id, changeset.blobs.length); // <64 hex> 1
// Upload with client.objects.upload({ repo, objects: [...changeset.blobs, changeset.object] }),
// then client.proposals.create({ repo, stream: 'main', head: changeset.id, title: '...' }).
// #endregion signing

/** What the test checks. */
export const signingResult = {
  blob: blobId('hello\n'),
  valid: verifyProvenance(record, signature, key.publicKey),
  tampered: verifyProvenance({ ...record, model: 'other' }, signature, key.publicKey),
  changesetId: changeset.id,
};
