// Ed25519 signing for the two signed record kinds a client can submit (see docs/protocol/agwp-v1.md
// "Signing formats" and docs/architecture/object-format.md "Signatures"). Node's built-in crypto only.
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  randomBytes,
  sign,
} from 'node:crypto'
import type { KeyObject } from 'node:crypto'

export interface KeyPair {
  privateKey: KeyObject
  /** 64 lowercase hex characters (raw 32-byte Ed25519 public key). */
  publicHex: string
}

export function generateKeyPair(): KeyPair {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519')
  const der = publicKey.export({ format: 'der', type: 'spki' })
  return { privateKey, publicHex: Buffer.from(der.subarray(der.length - 32)).toString('hex') }
}

/** A deterministic key (for repeatable demo data). */
export function keyFromSeed(seed: string): KeyPair {
  const raw = createHash('sha256').update(seed).digest()
  // PKCS#8 prefix for an Ed25519 private key.
  const pkcs8 = Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), raw])
  const privateKey = createPrivateKey({ key: pkcs8, format: 'der', type: 'pkcs8' })
  const publicKey = createPublicKey(privateKey)
  const der = publicKey.export({ format: 'der', type: 'spki' })
  return { privateKey, publicHex: Buffer.from(der.subarray(der.length - 32)).toString('hex') }
}

function signedMessage(domain: string, message: Uint8Array): Buffer {
  const d = Buffer.from(domain, 'utf8')
  return Buffer.concat([
    Buffer.from('agentgit.sig.v1\0', 'utf8'),
    Buffer.from([d.length]),
    d,
    message,
  ])
}

export function signDomain(key: KeyPair, domain: string, message: Uint8Array): string {
  return sign(null, signedMessage(domain, message), key.privateKey).toString('hex')
}

function varint(n: number): Buffer {
  const out: number[] = []
  let v = n
  for (;;) {
    const byte = v & 0x7f
    v = Math.floor(v / 128)
    if (v === 0) {
      out.push(byte)
      break
    }
    out.push(byte | 0x80)
  }
  return Buffer.from(out)
}

function str(s: string): Buffer {
  const b = Buffer.from(s, 'utf8')
  return Buffer.concat([varint(b.length), b])
}

function i64(n: number): Buffer {
  const b = Buffer.alloc(8)
  b.writeBigInt64BE(BigInt(n))
  return b
}

export interface AttestationInput {
  treeId: string
  check: string
  result: 'pass' | 'fail'
  policyVersion: number
  environmentDigest?: string
  issuer: string
  issuedAtMs: number
}

/** The wire form of a signed attestation (JSON fields as in the OpenAPI `Attestation`). */
export function signAttestation(key: KeyPair, a: AttestationInput) {
  const id = `att_${randomBytes(10).toString('hex')}`
  const env = a.environmentDigest ?? ''
  const msg = Buffer.concat([
    str(id),
    Buffer.from(a.treeId, 'hex'),
    str(a.check),
    Buffer.from([a.result === 'pass' ? 1 : 2]),
    varint(a.policyVersion),
    str(env),
    str(a.issuer),
    i64(a.issuedAtMs),
  ])
  return {
    id,
    tree_id: a.treeId,
    check: a.check,
    result: a.result,
    policy_version: a.policyVersion,
    environment_digest: env,
    issuer: a.issuer,
    issued_at_ms: a.issuedAtMs,
    public_key: key.publicHex,
    signature: signDomain(key, 'attestation.v1', msg),
  }
}

function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys)
  if (v && typeof v === 'object') {
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, x]) => [k, sortKeys(x)]),
    )
  }
  return v
}

export interface ProvenanceInput {
  changeset: string
  agent: string
  runId?: string
  taskId?: string
  model?: string
  promptDigest?: string
  parameters?: Record<string, string>
  checksRun?: string[]
  inputs?: string[]
  rationale?: string
  createdAtMs: number
}

/** Returns `{record, signature}` ready for POST /provenance. */
export function signProvenance(key: KeyPair, p: ProvenanceInput) {
  const canonical = {
    agent: p.agent,
    changeset: p.changeset,
    checks_run: p.checksRun ?? [],
    created_at: p.createdAtMs,
    inputs: p.inputs ?? [],
    model: p.model ?? null,
    parameters: p.parameters ?? {},
    prompt_digest: p.promptDigest ?? null,
    rationale: p.rationale ?? null,
    run_id: p.runId ?? null,
    schema: 'agentgit.provenance.v1',
    task_id: p.taskId ?? null,
  }
  const bytes = Buffer.from(JSON.stringify(sortKeys(canonical)), 'utf8')
  const record: Record<string, unknown> = {
    changeset: p.changeset,
    agent: p.agent,
    created_at_ms: p.createdAtMs,
  }
  if (p.runId) record.run_id = p.runId
  if (p.taskId) record.task_id = p.taskId
  if (p.model) record.model = p.model
  if (p.promptDigest) record.prompt_digest = p.promptDigest
  if (p.parameters) record.parameters = p.parameters
  if (p.checksRun?.length) record.checks_run = p.checksRun
  if (p.inputs?.length) record.inputs = p.inputs
  if (p.rationale) record.rationale = p.rationale
  return { record, signature: signDomain(key, 'provenance.v1', bytes) }
}

export const b64 = (s: string | Uint8Array): string => Buffer.from(s).toString('base64')
