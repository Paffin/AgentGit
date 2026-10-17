import { sha256 } from '@noble/hashes/sha2.js';
import { concatBytes, toHex, uint, utf8Encode } from './encoding.js';

/** Object kinds of the content-addressed store (see docs/architecture/object-format.md). */
export const ObjectKind = { blob: 1, changeset: 2, tree: 3 } as const;
export type ObjectKindName = keyof typeof ObjectKind;

/** Current format version of every object kind. */
export const OBJECT_VERSION = 1;

const ID_DOMAIN = utf8Encode('agentgit.object\0');
const ENVELOPE_MAGIC = utf8Encode('AGOB');

/**
 * `SHA-256("agentgit.object\0" || kind:u8 || version:u16be || length:u64be || payload)`, lowercase hex.
 * The kind and version are part of the preimage, so a blob never collides with a changeset.
 */
export function objectId(kind: number, version: number, payload: Uint8Array): string {
  const h = sha256.create();
  h.update(ID_DOMAIN);
  h.update(uint(kind, 1));
  h.update(uint(version, 2));
  h.update(uint(payload.length, 8));
  h.update(payload);
  return toHex(h.digest());
}

/** The content id of a blob (what `FileChange.blob_id` and changeset puts refer to). Text is UTF-8 encoded. */
export function blobId(content: Uint8Array | string): string {
  return objectId(ObjectKind.blob, OBJECT_VERSION, typeof content === 'string' ? utf8Encode(content) : content);
}

/** The stored form of an object: `"AGOB" || kind:u8 || version:u16be || payload`. */
export function encodeEnvelope(kind: number, payload: Uint8Array, version: number = OBJECT_VERSION): Uint8Array {
  return concatBytes(ENVELOPE_MAGIC, uint(kind, 1), uint(version, 2), payload);
}

/** Splits an envelope; throws `TypeError` when it is not one. */
export function decodeEnvelope(envelope: Uint8Array): { kind: number; version: number; payload: Uint8Array } {
  if (envelope.length < 7 || !ENVELOPE_MAGIC.every((b, i) => envelope[i] === b)) {
    throw new TypeError('not an AgentGit object envelope');
  }
  return { kind: envelope[4]!, version: (envelope[5]! << 8) | envelope[6]!, payload: envelope.subarray(7) };
}

/** The envelope of a blob, ready for `objects.put`, together with its id. */
export function blobObject(content: Uint8Array | string): { id: string; envelope: Uint8Array } {
  const payload = typeof content === 'string' ? utf8Encode(content) : content;
  return {
    id: objectId(ObjectKind.blob, OBJECT_VERSION, payload),
    envelope: encodeEnvelope(ObjectKind.blob, payload),
  };
}
