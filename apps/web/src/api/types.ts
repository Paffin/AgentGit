// Friendly aliases over the types generated from docs/protocol/openapi.json (see `npm run gen:api`).
// Nothing here is hand-written wire format: if the protocol changes, regenerate and fix the compile errors.
import type { components } from './schema.gen'

type S = components['schemas']

export type Principal = S['Principal']
export type CredentialScope = S['CredentialScope']
export type Capabilities = S['NegotiateResponse']
export type Repo = S['Repo']
export type Quota = S['Quota']
export type Stream = S['Stream']
export type ReflogEntry = S['ReflogEntry']
export type Changeset = S['Changeset']
export type FileOp = S['FileOp']
export type DirEntry = S['DirEntry']
export type TreeEntry = S['TreeEntry']
export type FileDiff = S['FileDiff']
export type DiffResponse = S['DiffResponse']
export type Proposal = S['Proposal']
export type Review = S['Review']
export type ProposalDetail = S['ProposalDetail']
export type Preparation = S['Preparation']
export type Blocker = S['Blocker']
export type Conflict = S['Conflict']
export type ValidationStatus = S['ValidationStatus']
export type Attestation = S['Attestation']
export type IntegrationOutcome = S['IntegrationOutcome']
export type IntegrationResult = S['IntegrationResult']
export type QueueEntry = S['QueueEntry']
export type Job = S['Job']
export type Agent = S['Agent']
export type Run = S['Run']
export type Task = S['Task']
export type Lease = S['Lease']
export type ProvenanceEntry = S['ProvenanceEntry']
export type ProvenanceRecord = S['ProvenanceRecord']
export type Overlap = S['Overlap']
export type ServerEvent = S['Event']
export type PolicyVersion = S['PolicyVersion']
export type Pin = S['Pin']
export type AuditEntry = S['AuditEntry']
export type GcResponse = S['GcResponse']
export type FsckResponse = S['FsckResponse']
export type Credential = S['Credential']
export type RoleBinding = S['RoleBinding']
export type Operation = S['Operation']

/** Request bodies: every field is optional on the wire (missing = zero value), so callers pass only what they mean. */
export type Req<K extends keyof S> = Partial<S[K]>

export type Role = 'reader' | 'writer' | 'reviewer' | 'integrator' | 'runner' | 'admin'
export const ROLES: readonly Role[] = [
  'reader',
  'writer',
  'reviewer',
  'integrator',
  'runner',
  'admin',
]

export type IdentityKind = 'user' | 'agent' | 'service' | 'system' | 'unknown'

export interface Page<T> {
  items: T[]
  nextPageToken: string
}
