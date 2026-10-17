// The helpers must produce exactly the bytes of the independent reference implementation
// (tests/conformance/gen_vectors.py) and of sdk/testdata/gen_signing_vectors.py. No server needed.

import { createPrivateKey, createPublicKey, sign as nodeSign, verify as nodeVerify } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import {
  agentKeyFromSeed,
  attestationSigningBytes,
  blobId,
  buildChangeset,
  canonicalJson,
  canonicalProvenanceBytes,
  decodeBase64,
  decodeEnvelope,
  encodeBase64,
  encodeEnvelope,
  fromHex,
  generateAgentKey,
  objectId,
  ObjectKind,
  signAttestation,
  signingMessage,
  signProvenance,
  toHex,
  verifyProvenance,
  type ChangesetOpInput,
  type ProvenanceRecordInput,
} from '../src/index.js';
import { varint, utf8Encode, lengthPrefixedString, concatBytes } from '../src/encoding.js';
import { conformanceVectors, signingVectors } from './harness/vectors.js';

const vectors = conformanceVectors();

describe('blob ids', () => {
  it.each(vectors.blobs.map((b) => [JSON.stringify(b.content_utf8), b] as const))(
    'matches the vector for %s',
    (_n, b) => {
      expect(blobId(b.content_utf8)).toBe(b.id);
      expect(blobId(utf8Encode(b.content_utf8))).toBe(b.id);
    },
  );

  it('keeps kinds apart: the same bytes as a changeset payload have another id', () => {
    const bytes = utf8Encode('same bytes');
    expect(objectId(ObjectKind.blob, 1, bytes)).not.toBe(objectId(ObjectKind.changeset, 1, bytes));
    expect(objectId(ObjectKind.blob, 1, bytes)).not.toBe(objectId(ObjectKind.blob, 2, bytes));
  });

  it('round-trips the object envelope', () => {
    const env = encodeEnvelope(ObjectKind.blob, utf8Encode('hello\n'));
    expect(toHex(env.subarray(0, 7))).toBe('41474f42010001');
    expect(decodeEnvelope(env)).toMatchObject({ kind: 1, version: 1 });
    expect(() => decodeEnvelope(utf8Encode('nope'))).toThrow(TypeError);
  });
});

describe('tree manifests', () => {
  // Trees are derived server-side; their ids are still defined by the same content-id function.
  it.each(vectors.trees.map((t) => [t.name, t] as const))('tree "%s" gets the id of the vector', (_n, t) => {
    expect(objectId(ObjectKind.tree, 1, fromHex(t.payload_hex))).toBe(t.id);
  });
});

describe('changesets', () => {
  function inputOf(v: (typeof vectors.changesets)[number]) {
    const ops: ChangesetOpInput[] = v.ops.map((o) =>
      o.op === 'delete'
        ? { op: 'delete', path: o.path }
        : {
            op: 'put',
            path: o.path,
            content: o.content ?? '',
            fileType: o.type === 'symlink' ? 'symlink' : 'regular',
            executable: o.executable === true,
          },
    );
    return {
      namespace: v.namespace,
      parents: v.parents,
      author: v.author,
      timestampMs: v.timestamp_ms,
      message: v.message,
      ops,
    };
  }

  it.each(vectors.changesets.map((v) => [v.name, v] as const))('encodes "%s" byte for byte', (_n, v) => {
    const built = buildChangeset(inputOf(v));
    expect(toHex(built.payload)).toBe(v.payload_hex);
    expect(built.id).toBe(v.id);
    expect(toHex(built.object.envelope.subarray(7))).toBe(v.payload_hex);
  });

  it('collects each blob once, so it can be uploaded before the changeset', () => {
    const built = buildChangeset({
      namespace: 'acme/web',
      author: 'user:alice',
      timestampMs: 1,
      ops: [
        { op: 'put', path: 'a.txt', content: 'same' },
        { op: 'put', path: 'b.txt', content: 'same' },
        { op: 'put', path: 'c.txt', content: 'other' },
      ],
    });
    expect(built.blobs.map((b) => b.id).sort()).toEqual([blobId('same'), blobId('other')].sort());
  });

  it('rejects what the format forbids', () => {
    const base = { namespace: 'acme/web', author: 'user:alice', timestampMs: 1 };
    const put = (path: string): ChangesetOpInput => ({ op: 'put', path, content: 'x' });
    expect(() => buildChangeset({ ...base, ops: [put('../etc/passwd')] })).toThrow(/invalid path/);
    expect(() => buildChangeset({ ...base, ops: [put('/abs')] })).toThrow(/relative/);
    expect(() => buildChangeset({ ...base, ops: [put('a/.git/config')] })).toThrow(/reserved/);
    expect(() => buildChangeset({ ...base, ops: [put('a\\b')] })).toThrow(/backslash/);
    expect(() => buildChangeset({ ...base, ops: [put('a'), put('a')] })).toThrow(/twice/);
    expect(() => buildChangeset({ ...base, author: 'bob', ops: [] })).toThrow(/identity/);
    expect(() => buildChangeset({ ...base, namespace: 'Acme/Web', ops: [] })).toThrow(/namespace/);
    expect(() => buildChangeset({ ...base, parents: ['zz'], ops: [] })).toThrow(/parent/);
    expect(() =>
      buildChangeset({ ...base, ops: [{ op: 'put', path: 'l', content: 'x', fileType: 'symlink', executable: true }] }),
    ).toThrow(/symlink/);
  });

  it('sorts operations by path bytes, not by the order given', () => {
    const a = buildChangeset({
      namespace: 'acme/web',
      author: 'user:alice',
      timestampMs: 1,
      ops: [
        { op: 'put', path: 'b', content: '1' },
        { op: 'put', path: 'a/z', content: '2' },
        { op: 'delete', path: 'a/a' },
      ],
    });
    const b = buildChangeset({
      namespace: 'acme/web',
      author: 'user:alice',
      timestampMs: 1,
      ops: [
        { op: 'delete', path: 'a/a' },
        { op: 'put', path: 'a/z', content: '2' },
        { op: 'put', path: 'b', content: '1' },
      ],
    });
    expect(a.id).toBe(b.id);
  });
});

describe('attestation signing bytes', () => {
  it.each(vectors.attestations.map((a, i) => [`#${i} (${a.result})`, a] as const))('matches the vector %s', (_n, a) => {
    expect(toHex(attestationSigningBytes(a))).toBe(a.signing_bytes_hex);
  });

  it('produces a signature an independent verifier (OpenSSL) accepts', () => {
    const key = generateAgentKey();
    const a = vectors.attestations[0]!;
    const signed = signAttestation(a, key);
    const message = signingMessage('attestation.v1', fromHex(a.signing_bytes_hex));
    expect(nodeVerify(null, message, nodePublicKey(key.publicKey), fromHex(signed.signature))).toBe(true);
    expect(signed.public_key).toBe(key.publicKeyHex);
  });
});

function nodePublicKey(raw: Uint8Array) {
  const spkiPrefix = fromHex('302a300506032b6570032100');
  return createPublicKey({ key: Buffer.concat([spkiPrefix, raw]), format: 'der', type: 'spki' });
}

function nodePrivateKey(seed: Uint8Array) {
  const pkcs8Prefix = fromHex('302e020100300506032b657004220420');
  return createPrivateKey({ key: Buffer.concat([pkcs8Prefix, seed]), format: 'der', type: 'pkcs8' });
}

describe('provenance signing', () => {
  const sv = signingVectors();
  const key = agentKeyFromSeed(sv.seed_hex);

  it('derives the public key of the vector seed', () => {
    expect(key.publicKeyHex).toBe(sv.public_key_hex);
  });

  it.each(sv.cases.map((c) => [c.name, c] as const))(
    'case "%s": canonical bytes, signed message and signature',
    (_n, c) => {
      const record = c.record as unknown as ProvenanceRecordInput;
      expect(new TextDecoder().decode(canonicalProvenanceBytes(record))).toBe(c.canonical_json);
      expect(toHex(signingMessage('provenance.v1', canonicalProvenanceBytes(record)))).toBe(c.signed_message_hex);
      // Ed25519 is deterministic: the signature equals the one OpenSSL made in the generator.
      expect(signProvenance(record, key.privateKey)).toBe(c.signature_hex);
      expect(verifyProvenance(record, c.signature_hex, key.publicKey)).toBe(true);
    },
  );

  it('agrees with OpenSSL in both directions, for random keys', () => {
    const record = sv.cases[1]!.record as unknown as ProvenanceRecordInput;
    for (let i = 0; i < 5; i++) {
      const k = generateAgentKey();
      const ours = signProvenance(record, k.privateKey);
      const message = signingMessage('provenance.v1', canonicalProvenanceBytes(record));
      expect(nodeVerify(null, message, nodePublicKey(k.publicKey), fromHex(ours))).toBe(true);
      expect(toHex(nodeSign(null, message, nodePrivateKey(k.privateKey)))).toBe(ours);
    }
  });

  it('rejects a tampered record, a foreign key and a flipped signature bit', () => {
    const record = { ...(sv.cases[0]!.record as unknown as ProvenanceRecordInput) };
    const sig = signProvenance(record, key.privateKey);
    expect(verifyProvenance({ ...record, model: 'other' }, sig, key.publicKey)).toBe(false);
    expect(verifyProvenance(record, sig, generateAgentKey().publicKey)).toBe(false);
    const flipped = sig.slice(0, -1) + (sig.endsWith('0') ? '1' : '0');
    expect(verifyProvenance(record, flipped, key.publicKey)).toBe(false);
    expect(verifyProvenance(record, 'not hex', key.publicKey)).toBe(false);
  });

  it('is domain separated: a provenance signature does not verify as an attestation', () => {
    const record = sv.cases[0]!.record as unknown as ProvenanceRecordInput;
    const sig = signProvenance(record, key.privateKey);
    const bytes = canonicalProvenanceBytes(record);
    expect(nodeVerify(null, signingMessage('attestation.v1', bytes), nodePublicKey(key.publicKey), fromHex(sig))).toBe(
      false,
    );
    expect(nodeVerify(null, bytes, nodePublicKey(key.publicKey), fromHex(sig))).toBe(false);
  });

  it('sorts keys by UTF-8 bytes at every level', () => {
    expect(canonicalJson({ b: 1, a: { d: [3, { y: 1, x: 2 }], c: null } })).toBe(
      '{"a":{"c":null,"d":[3,{"x":2,"y":1}]},"b":1}',
    );
    expect(() => canonicalJson({ n: 1.5 })).toThrow(RangeError);
  });
});

describe('encoding helpers', () => {
  it('round-trips base64 for all lengths and matches Node', () => {
    for (let n = 0; n < 40; n++) {
      const bytes = Uint8Array.from({ length: n }, (_, i) => (i * 37 + n) & 0xff);
      const text = encodeBase64(bytes);
      expect(text).toBe(Buffer.from(bytes).toString('base64'));
      expect(Array.from(decodeBase64(text))).toEqual(Array.from(bytes));
    }
    expect(Array.from(decodeBase64('aGk'))).toEqual([104, 105]); // padding optional
    expect(() => decodeBase64('a$b=')).toThrow(TypeError);
    expect(() => decodeBase64('a')).toThrow(TypeError);
  });

  it('encodes minimal LEB128 varints', () => {
    expect(toHex(varint(0))).toBe('00');
    expect(toHex(varint(127))).toBe('7f');
    expect(toHex(varint(128))).toBe('8001');
    expect(toHex(varint(300))).toBe('ac02');
    expect(toHex(lengthPrefixedString('é'))).toBe('02c3a9');
    expect(toHex(concatBytes(Uint8Array.of(1), Uint8Array.of(2)))).toBe('0102');
  });
});
