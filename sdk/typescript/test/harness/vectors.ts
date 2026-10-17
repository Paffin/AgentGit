import { readFileSync } from 'node:fs';
import path from 'node:path';
import { REPO_ROOT } from './server.js';

export interface ConformanceVectors {
  format: string;
  blobs: { content_utf8: string; id: string }[];
  changesets: {
    name: string;
    namespace: string;
    parents: string[];
    author: string;
    timestamp_ms: number;
    message: string;
    ops: { op: 'put' | 'delete'; path: string; content?: string; type?: string; executable?: boolean }[];
    payload_hex: string;
    id: string;
  }[];
  trees: {
    name: string;
    entries: { path: string; content: string; type?: string; executable?: boolean }[];
    payload_hex: string;
    id: string;
  }[];
  attestations: {
    id: string;
    tree_id: string;
    check: string;
    result: 'pass' | 'fail';
    policy_version: number;
    environment_digest: string;
    issuer: string;
    issued_at_ms: number;
    signing_bytes_hex: string;
  }[];
}

/** The independent vectors of tests/conformance (written by a from-scratch Python encoder). */
export function conformanceVectors(): ConformanceVectors {
  return JSON.parse(readFileSync(path.join(REPO_ROOT, 'tests/conformance/vectors.json'), 'utf8')) as ConformanceVectors;
}

export interface SigningVectors {
  seed_hex: string;
  public_key_hex: string;
  cases: {
    name: string;
    record: Record<string, unknown>;
    canonical_json: string;
    signed_message_hex: string;
    signature_hex: string;
  }[];
}

/** Provenance vectors produced by sdk/testdata/gen_signing_vectors.py (plain json + `cryptography`). */
export function signingVectors(): SigningVectors {
  return JSON.parse(readFileSync(path.join(REPO_ROOT, 'sdk/testdata/signing-vectors.json'), 'utf8')) as SigningVectors;
}
