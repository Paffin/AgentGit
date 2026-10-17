import { concatBytes, fromHex, int64, lengthPrefixedString, uint, utf8Encode, varint } from './encoding.js';
import { blobId, encodeEnvelope, ObjectKind, objectId, OBJECT_VERSION } from './hashing.js';

/** Limits of `ChangesetV1` (docs/architecture/object-format.md). */
export const CHANGESET_LIMITS = { maxParents: 16, maxOps: 100_000, maxMessageBytes: 64 * 1024 } as const;

export type FileType = 'regular' | 'symlink';

/** One operation of a changeset. For a `put`, give the bytes in `content` or an existing blob id in `blob`. */
export type ChangesetOpInput =
  | {
      op: 'put';
      path: string;
      /** File bytes (or the link target for a symlink). Text is UTF-8 encoded. */
      content?: Uint8Array | string;
      /** Id of a blob that is already stored (instead of `content`). */
      blob?: string;
      fileType?: FileType;
      executable?: boolean;
    }
  | { op: 'delete'; path: string };

export interface ChangesetInput {
  /** The repository's namespace (`Repo.namespace`, normally equal to its name). It is part of the hash. */
  namespace: string;
  /** Parent changeset ids (hex); none for a root. `ops` are relative to the first parent. */
  parents?: string[];
  /** Identity, for example `agent:bot#run_ab12`. */
  author: string;
  /** Author time, Unix milliseconds. */
  timestampMs: number;
  message?: string;
  ops: ChangesetOpInput[];
}

export interface StoredObject {
  id: string;
  /** The bytes for `objects.put` (`"AGOB" || kind || version || payload`). */
  envelope: Uint8Array;
}

export interface BuiltChangeset {
  /** The changeset id: what you propose and what other changesets name as parent. */
  id: string;
  /** The canonical `ChangesetV1` payload. */
  payload: Uint8Array;
  /** The changeset object itself, ready for `objects.put`. */
  object: StoredObject;
  /** The blobs created from `content`, de-duplicated; upload these before the changeset. */
  blobs: StoredObject[];
}

const IDENTITY = /^(user|agent|service|system):[A-Za-z0-9._@#+-]{1,128}$/;
const NAMESPACE_SEGMENT = /^[a-z0-9][a-z0-9._-]*$/;
const HEX64 = /^[0-9a-f]{64}$/;

/** Throws `TypeError` unless `path` is a valid repository path (docs/architecture/object-format.md, "Value types"). */
export function assertRepoPath(path: string): void {
  const bytes = utf8Encode(path);
  if (bytes.length === 0 || bytes.length > 4096) throw new TypeError(`invalid path "${path}": 1..=4096 bytes`);
  if (path.startsWith('/')) throw new TypeError(`invalid path "${path}": must be relative`);
  for (const part of path.split('/')) {
    if (part === '' || part === '.' || part === '..')
      throw new TypeError(`invalid path "${path}": empty, "." or ".." component`);
    if (utf8Encode(part).length > 255) throw new TypeError(`invalid path "${path}": a component exceeds 255 bytes`);
    const lower = part.toLowerCase();
    if (lower === '.git' || lower === '.ag') throw new TypeError(`invalid path "${path}": ${part} is reserved`);
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f\\]/.test(path))
    throw new TypeError(`invalid path "${path}": control character or backslash`);
}

function assertNamespace(ns: string): void {
  if (ns.length === 0 || ns.length > 128 || !ns.split('/').every((s) => NAMESPACE_SEGMENT.test(s))) {
    throw new TypeError(`invalid namespace "${ns}"`);
  }
}

const compareBytes = (a: Uint8Array, b: Uint8Array): number => {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return a[i]! - b[i]!;
  return a.length - b.length;
};

/**
 * Encodes a `ChangesetV1` exactly as `docs/architecture/object-format.md` specifies (operations are
 * sorted by path bytes for you) and returns its id together with the objects to upload.
 */
export function buildChangeset(input: ChangesetInput): BuiltChangeset {
  assertNamespace(input.namespace);
  if (!IDENTITY.test(input.author)) throw new TypeError(`invalid author identity "${input.author}"`);
  const parents = input.parents ?? [];
  if (parents.length > CHANGESET_LIMITS.maxParents) throw new TypeError('too many parents');
  if (new Set(parents).size !== parents.length) throw new TypeError('duplicate parent');
  for (const p of parents) if (!HEX64.test(p)) throw new TypeError(`invalid parent id "${p}"`);
  const message = input.message ?? '';
  if (utf8Encode(message).length > CHANGESET_LIMITS.maxMessageBytes) throw new TypeError('message too long');
  if (!Number.isSafeInteger(input.timestampMs)) throw new TypeError('timestampMs must be an integer');
  if (input.ops.length > CHANGESET_LIMITS.maxOps) throw new TypeError('too many operations');

  const blobs = new Map<string, StoredObject>();
  const encoded = input.ops.map((op) => {
    assertRepoPath(op.path);
    const key = utf8Encode(op.path);
    if (op.op === 'delete') {
      return { key, bytes: concatBytes(Uint8Array.of(2), lengthPrefixedString(op.path)) };
    }
    const fileType = op.fileType ?? 'regular';
    if (op.executable && fileType === 'symlink') throw new TypeError(`"${op.path}": a symlink cannot be executable`);
    let id = op.blob;
    if (op.content !== undefined) {
      if (id !== undefined) throw new TypeError(`"${op.path}": give either content or blob, not both`);
      const payload = typeof op.content === 'string' ? utf8Encode(op.content) : op.content;
      id = blobId(payload);
      if (!blobs.has(id)) blobs.set(id, { id, envelope: encodeEnvelope(ObjectKind.blob, payload) });
    }
    if (id === undefined || !HEX64.test(id)) throw new TypeError(`"${op.path}": a put needs content or a blob id`);
    return {
      key,
      bytes: concatBytes(
        Uint8Array.of(1),
        lengthPrefixedString(op.path),
        Uint8Array.of(fileType === 'symlink' ? 1 : 0, op.executable ? 1 : 0),
        fromHex(id),
      ),
    };
  });
  encoded.sort((a, b) => compareBytes(a.key, b.key));
  for (let i = 1; i < encoded.length; i++) {
    if (compareBytes(encoded[i - 1]!.key, encoded[i]!.key) === 0) throw new TypeError('a path appears twice');
  }

  const payload = concatBytes(
    uint(1, 2),
    lengthPrefixedString(input.namespace),
    varint(parents.length),
    ...parents.map(fromHex),
    lengthPrefixedString(input.author),
    int64(input.timestampMs),
    lengthPrefixedString(message),
    varint(encoded.length),
    ...encoded.map((e) => e.bytes),
  );
  const id = objectId(ObjectKind.changeset, OBJECT_VERSION, payload);
  return {
    id,
    payload,
    object: { id, envelope: encodeEnvelope(ObjectKind.changeset, payload) },
    blobs: [...blobs.values()],
  };
}
