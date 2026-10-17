import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2.js';
import {
  concatBytes,
  fromHex,
  int64,
  lengthPrefixedString,
  randomBytes,
  toHex,
  utf8Encode,
  varint,
} from './encoding.js';

// noble-ed25519 needs a SHA-512 for its synchronous API; supply the audited one from noble-hashes.
ed.hashes.sha512 = sha512;

const SIG_PREFIX = utf8Encode('agentgit.sig.v1\0');

/** Schema tag of a provenance record. */
export const PROVENANCE_SCHEMA = 'agentgit.provenance.v1';

/** An Ed25519 key pair for an agent. The private key never needs to leave the agent. */
export interface AgentKey {
  /** 32-byte Ed25519 seed. Keep it secret. */
  privateKey: Uint8Array;
  /** 32-byte public key. */
  publicKey: Uint8Array;
  /** Lowercase hex of the public key: what `agents.register` expects as `public_key`. */
  publicKeyHex: string;
}

function asSeed(privateKey: Uint8Array | string): Uint8Array {
  const seed = typeof privateKey === 'string' ? fromHex(privateKey) : privateKey;
  if (seed.length !== 32) throw new TypeError('an Ed25519 private key is 32 bytes');
  return seed;
}

/** A fresh random key pair. */
export function generateAgentKey(): AgentKey {
  return agentKeyFromSeed(randomBytes(32));
}

/** Rebuilds a key pair from a 32-byte seed (bytes or hex). */
export function agentKeyFromSeed(seed: Uint8Array | string): AgentKey {
  const privateKey = asSeed(seed);
  const publicKey = ed.getPublicKey(privateKey);
  return { privateKey, publicKey, publicKeyHex: toHex(publicKey) };
}

/** `"agentgit.sig.v1\0" || len(domain):u8 || domain || message` - the bytes that are actually signed. */
export function signingMessage(domain: string, message: Uint8Array): Uint8Array {
  const d = utf8Encode(domain);
  if (d.length === 0 || d.length > 64) throw new RangeError('a signature domain is 1..=64 bytes');
  return concatBytes(SIG_PREFIX, Uint8Array.of(d.length), d, message);
}

/** Ed25519 signature (lowercase hex) of `message` under `domain`. */
export function signWithDomain(domain: string, message: Uint8Array, privateKey: Uint8Array | string): string {
  return toHex(ed.sign(signingMessage(domain, message), asSeed(privateKey)));
}

/** Verifies a hex signature with the strict RFC 8032 rules the server applies. */
export function verifyWithDomain(
  domain: string,
  message: Uint8Array,
  signatureHex: string,
  publicKey: Uint8Array | string,
): boolean {
  try {
    const pk = typeof publicKey === 'string' ? fromHex(publicKey) : publicKey;
    return ed.verify(fromHex(signatureHex), signingMessage(domain, message), pk, { zip215: false });
  } catch {
    return false;
  }
}

/** A provenance record as sent on the wire (`ProvenanceRecord` of the protocol). Empty/absent optionals mean "not set". */
export interface ProvenanceRecordInput {
  /** The changeset described (64 hex characters). */
  changeset: string;
  /** The acting identity, for example `agent:refactor-bot#run_ab12`. */
  agent: string;
  run_id?: string;
  task_id?: string;
  model?: string;
  /** SHA-256 hex of the prompt; the prompt itself is not stored. */
  prompt_digest?: string;
  parameters?: Record<string, string>;
  checks_run?: string[];
  /** Sources consulted. Untrusted text. */
  inputs?: string[];
  rationale?: string;
  /** Unix milliseconds. */
  created_at_ms: number;
}

const compareUtf8 = (a: string, b: string): number => {
  const x = utf8Encode(a);
  const y = utf8Encode(b);
  const n = Math.min(x.length, y.length);
  for (let i = 0; i < n; i++) if (x[i] !== y[i]) return x[i]! - y[i]!;
  return x.length - y.length;
};

/** Compact JSON with keys sorted by UTF-8 bytes at every level - the representation the server signs and verifies. */
export function canonicalJson(value: unknown): string {
  if (value === null) return 'null';
  switch (typeof value) {
    case 'string':
      return JSON.stringify(value);
    case 'boolean':
      return value ? 'true' : 'false';
    case 'number':
      if (!Number.isSafeInteger(value)) throw new RangeError('canonical JSON carries integers only');
      return String(value);
    case 'bigint':
      return value.toString();
    case 'object': {
      if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
      const entries = Object.entries(value as Record<string, unknown>).filter(([, v]) => v !== undefined);
      entries.sort(([a], [b]) => compareUtf8(a, b));
      return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${canonicalJson(v)}`).join(',')}}`;
    }
    default:
      throw new TypeError(`cannot encode a ${typeof value} as canonical JSON`);
  }
}

/**
 * The bytes the server verifies for a provenance record: compact JSON, keys sorted at every level,
 * unset optionals as `null`, `created_at_ms` as `created_at`, plus the schema tag.
 */
export function canonicalProvenanceBytes(record: ProvenanceRecordInput): Uint8Array {
  if (!Number.isSafeInteger(record.created_at_ms)) throw new TypeError('created_at_ms must be an integer');
  const opt = (s: string | undefined): string | null => (s === undefined || s === '' ? null : s);
  return utf8Encode(
    canonicalJson({
      schema: PROVENANCE_SCHEMA,
      changeset: record.changeset,
      agent: record.agent,
      run_id: opt(record.run_id),
      task_id: opt(record.task_id),
      model: opt(record.model),
      prompt_digest: opt(record.prompt_digest),
      parameters: record.parameters ?? {},
      checks_run: record.checks_run ?? [],
      inputs: record.inputs ?? [],
      rationale: opt(record.rationale),
      created_at: record.created_at_ms,
    }),
  );
}

/** Signs a provenance record with the agent's own key; the result goes into `provenance.submit({ signature })`. */
export function signProvenance(record: ProvenanceRecordInput, privateKey: Uint8Array | string): string {
  return signWithDomain('provenance.v1', canonicalProvenanceBytes(record), privateKey);
}

/** Checks a provenance signature the way the server does. */
export function verifyProvenance(
  record: ProvenanceRecordInput,
  signatureHex: string,
  publicKey: Uint8Array | string,
): boolean {
  return verifyWithDomain('provenance.v1', canonicalProvenanceBytes(record), signatureHex, publicKey);
}

/** An attestation as sent on the wire (`Attestation` of the protocol), without the key and signature. */
export interface AttestationInput {
  id: string;
  /** The exact tree that was checked (64 hex characters). */
  tree_id: string;
  check: string;
  result: 'pass' | 'fail';
  policy_version: number;
  environment_digest: string;
  issuer: string;
  issued_at_ms: number;
}

/** Canonical signing bytes of an attestation (docs/architecture/object-format.md, "Signatures"). */
export function attestationSigningBytes(a: AttestationInput): Uint8Array {
  return concatBytes(
    lengthPrefixedString(a.id),
    fromHex(a.tree_id),
    lengthPrefixedString(a.check),
    Uint8Array.of(a.result === 'pass' ? 1 : 2),
    varint(a.policy_version),
    lengthPrefixedString(a.environment_digest),
    lengthPrefixedString(a.issuer),
    int64(a.issued_at_ms),
  );
}

/** Signs an attestation as a trusted worker; returns the full `Attestation` message for `integration.attest`. */
export function signAttestation(
  a: AttestationInput,
  key: AgentKey,
): AttestationInput & { public_key: string; signature: string } {
  return {
    ...a,
    public_key: key.publicKeyHex,
    signature: signWithDomain('attestation.v1', attestationSigningBytes(a), key.privateKey),
  };
}
