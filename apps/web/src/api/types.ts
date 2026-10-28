// Friendly aliases over the types generated from docs/protocol/openapi.json (see `npm run gen:api`).
// Nothing here is hand-written wire format: if the protocol changes, regenerate and fix the compile errors.
import type { components } from './schema.gen'

type S = components['schemas']

export type Principal = S['Principal']
export type CredentialScope = S['CredentialScope']
export type Capabilities = S['NegotiateResponse']
export type AuthConfig = S['AuthConfig']
export type AuthProvider = S['AuthProvider']
export type Repo = S['Repo']
export type Quota = S['Quota']
export type Stream = S['Stream']
export type ReflogEntry = S['ReflogEntry']
export type Changeset = S['Changeset']
export type FileOp = S['FileOp']
export type DirEntry = S['DirEntry']
export type BlameLine = S['BlameLine']
export type BlameResponse = S['BlameResponse']
export type TreeEntry = S['TreeEntry']
export type FileDiff = S['FileDiff']
export type DiffResponse = S['DiffResponse']
export type SearchTreeResponse = S['SearchTreeResponse']
export type SearchMatch = S['SearchMatch']
export type Proposal = S['Proposal']
export type Review = S['Review']
export type ProposalDetail = S['ProposalDetail']
/**
 * The OpenAPI document describes free-form JSON objects (`type: object` with no properties), which the
 * generator renders as `Record<string, never>`. They are open-ended on the wire, so say so.
 */
type OpenObject = Record<string, unknown>
export type Blocker = Omit<S['Blocker'], 'detail'> & { detail?: OpenObject }
export type Conflict = S['Conflict']
export type Workspace = S['Workspace']
export type SyncWorkspaceResponse = S['SyncWorkspaceResponse']
export type ValidationStatus = S['ValidationStatus']
export type Attestation = S['Attestation']
export type QueueEntry = S['QueueEntry']
export type Job = Omit<S['Job'], 'result'> & { result?: OpenObject | null }
export type Preparation = Omit<S['Preparation'], 'blockers' | 'jobs'> & {
  blockers: Blocker[]
  jobs: Job[]
}
export type IntegrationOutcome = Omit<S['IntegrationOutcome'], 'blockers'> & { blockers: Blocker[] }
export type IntegrationResult = Omit<S['IntegrationResult'], 'outcome'> & {
  outcome: IntegrationOutcome | null
}
export type Agent = S['Agent']
export type Run = S['Run']
export type Issue = S['Issue']
export type IssueActivity = S['IssueActivity']
export type Task = S['Task']
export type Lease = S['Lease']
export type ProvenanceEntry = S['ProvenanceEntry']
export type ProvenanceRecord = S['ProvenanceRecord']
export type Overlap = S['Overlap']
export type ServerEvent = Omit<S['Event'], 'payload'> & { payload?: OpenObject }
export type PolicyVersion = S['PolicyVersion']
export type Pin = S['Pin']
export type AuditEntry = Omit<S['AuditEntry'], 'details'> & { details?: OpenObject }
export type GcResponse = S['GcResponse']
export type FsckResponse = S['FsckResponse']
export type Credential = S['Credential']
export type RoleBinding = S['RoleBinding']
export type Operation = S['Operation']

/** Request bodies: every field is optional on the wire (missing = zero value), so callers pass only what they mean. */
export type Req<K extends keyof S> = Partial<S[K]>
/** A file change as a request carries it: only what the operation needs. */
export type FileChange = Req<'FileChange'>

export type Role =
  'reader' | 'writer' | 'reviewer' | 'integrator' | 'runner' | 'delegator' | 'admin'
export const ROLES: readonly Role[] = [
  'reader',
  'writer',
  'reviewer',
  'integrator',
  'runner',
  'delegator',
  'admin',
]

export type IdentityKind = 'user' | 'agent' | 'service' | 'system' | 'unknown'

export interface Page<T> {
  items: T[]
  nextPageToken: string
}

export type TaskScheduling = S['TaskScheduling']
