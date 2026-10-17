// The four independent states of a proposal (docs/concepts.md): Mergeable · Validated · Approved · Integrated.
// None implies another, so each is derived on its own from `integration.prepare`, the reviews and the
// proposal itself — never from a single "ready" flag.
import type { Preparation, Proposal, Review } from '@/api/types'
import { parseIdentity } from './identity'

export type StateKey = 'mergeable' | 'validated' | 'approved' | 'integrated'
export type StateTone = 'ok' | 'pending' | 'blocked' | 'failed' | 'na' | 'unknown'

export interface StateInfo {
  key: StateKey
  name: string
  tone: StateTone
  /** Short value shown inside the badge: "clean", "1/2", "failed". */
  value: string
  /** One plain sentence for tooltips and screen readers. */
  summary: string
}

export type FourStates = Record<StateKey, StateInfo>

export interface ReviewSummary {
  /** Latest decision per reviewer principal, for the proposal's current head. */
  current: Review[]
  approvals: Review[]
  changesRequested: Review[]
  /** Approvals written against an older head: they no longer count. */
  staleApprovals: Review[]
}

export function summarizeReviews(reviews: readonly Review[], head: string): ReviewSummary {
  const latest = new Map<string, Review>()
  for (const r of [...reviews].sort((a, b) => a.id - b.id)) {
    latest.set(parseIdentity(r.reviewer).principal, r)
  }
  const current: Review[] = []
  const stale: Review[] = []
  for (const r of latest.values()) {
    if (r.head === head) current.push(r)
    else if (r.decision === 'approve') stale.push(r)
  }
  return {
    current,
    approvals: current.filter((r) => r.decision === 'approve'),
    changesRequested: current.filter((r) => r.decision === 'request_changes'),
    staleApprovals: stale,
  }
}

function info(
  key: StateKey,
  name: string,
  tone: StateTone,
  value: string,
  summary: string,
): StateInfo {
  return { key, name, tone, value, summary }
}

export interface DeriveInput {
  proposal: Proposal
  prep?: Preparation | null
  reviews?: readonly Review[] | null
}

export function deriveFourStates({ proposal, prep, reviews }: DeriveInput): FourStates {
  const integrated = proposal.state === 'integrated'
  const closed = proposal.state === 'closed'
  const summary = reviews ? summarizeReviews(reviews, proposal.head) : null

  // ---- Integrated ---------------------------------------------------------------------------------
  const integratedInfo = integrated
    ? info(
        'integrated',
        'Integrated',
        'ok',
        'yes',
        'The stream was updated atomically with this change.',
      )
    : closed
      ? info(
          'integrated',
          'Integrated',
          'na',
          'closed',
          'This proposal was closed without being integrated.',
        )
      : info(
          'integrated',
          'Integrated',
          'pending',
          'not yet',
          'The stream has not been updated with this change.',
        )

  if (integrated) {
    const done = (key: StateKey, name: string): StateInfo =>
      info(
        key,
        name,
        'ok',
        'at integration',
        `${name} requirements were satisfied when the stream was updated.`,
      )
    return {
      mergeable: done('mergeable', 'Mergeable'),
      validated: done('validated', 'Validated'),
      approved: done('approved', 'Approved'),
      integrated: integratedInfo,
    }
  }
  if (closed) {
    const na = (key: StateKey, name: string): StateInfo =>
      info(key, name, 'na', 'n/a', 'The proposal is closed.')
    return {
      mergeable: na('mergeable', 'Mergeable'),
      validated: na('validated', 'Validated'),
      approved: na('approved', 'Approved'),
      integrated: integratedInfo,
    }
  }

  // ---- Mergeable ----------------------------------------------------------------------------------
  let mergeable: StateInfo
  if (!prep) {
    mergeable = info(
      'mergeable',
      'Mergeable',
      'unknown',
      'checking',
      'Computing the candidate tree…',
    )
  } else if (prep.problem) {
    mergeable = info('mergeable', 'Mergeable', 'failed', 'problem', prep.problem)
  } else if (prep.conflicts.length > 0) {
    const n = prep.conflicts.length
    mergeable = info(
      'mergeable',
      'Mergeable',
      'failed',
      `${n} ${n === 1 ? 'conflict' : 'conflicts'}`,
      `No single result tree: ${prep.conflicts.map((c) => c.path).join(', ')}.`,
    )
  } else if (prep.strategy === 'fast_forward') {
    mergeable = info(
      'mergeable',
      'Mergeable',
      'ok',
      'clean',
      'Fast-forward: the stream head is an ancestor of this change.',
    )
  } else if (prep.strategy === 'merge') {
    mergeable = info(
      'mergeable',
      'Mergeable',
      'ok',
      'clean merge',
      'The stream moved, and the deterministic three-way merge is clean.',
    )
  } else if (prep.strategy === 'already_integrated') {
    mergeable = info(
      'mergeable',
      'Mergeable',
      'ok',
      'already in',
      'The stream already contains this change.',
    )
  } else {
    mergeable = info(
      'mergeable',
      'Mergeable',
      'unknown',
      'unknown',
      'No candidate tree could be computed.',
    )
  }

  // ---- Validated ----------------------------------------------------------------------------------
  let validated: StateInfo
  if (!prep) {
    validated = info('validated', 'Validated', 'unknown', 'checking', 'Looking up required checks…')
  } else if (prep.validations.length === 0) {
    validated = info(
      'validated',
      'Validated',
      'na',
      'none required',
      'The stream’s policy requires no checks.',
    )
  } else {
    const states = prep.validations.map((v) => v.state)
    const passed = states.filter((s) => s === 'passed').length
    const total = states.length
    if (states.includes('failed')) {
      validated = info(
        'validated',
        'Validated',
        'failed',
        'failed',
        'A trusted check failed on this exact tree.',
      )
    } else if (passed === total) {
      validated = info(
        'validated',
        'Validated',
        'ok',
        `${passed}/${total} passed`,
        'Every required check has a trusted passing attestation for this exact tree.',
      )
    } else if (states.includes('pending')) {
      validated = info(
        'validated',
        'Validated',
        'pending',
        'running',
        'A validation job is queued or running for this tree.',
      )
    } else {
      const missing = prep.validations.filter((v) => v.state !== 'passed').map((v) => v.check)
      validated = info(
        'validated',
        'Validated',
        'blocked',
        `${passed}/${total} passed`,
        `No trusted passing attestation for this tree: ${missing.join(', ')}.`,
      )
    }
  }

  // ---- Approved -----------------------------------------------------------------------------------
  let approved: StateInfo
  const approvalBlocker = prep?.blockers.find((b) => b.code === 'insufficient_approvals')
  const changesBlocker = prep?.blockers.find((b) => b.code === 'changes_requested')
  if (!prep && !summary) {
    approved = info('approved', 'Approved', 'unknown', 'checking', 'Counting approvals…')
  } else if (changesBlocker || (summary && summary.changesRequested.length > 0 && !prep)) {
    approved = info(
      'approved',
      'Approved',
      'failed',
      'changes requested',
      'An eligible reviewer asked for changes.',
    )
  } else if (approvalBlocker) {
    const d = (approvalBlocker.detail ?? {}) as { have?: number; required?: number }
    const have = typeof d.have === 'number' ? d.have : (summary?.approvals.length ?? 0)
    const required = typeof d.required === 'number' ? d.required : have + 1
    approved = info(
      'approved',
      'Approved',
      'blocked',
      `${have}/${required}`,
      `${have} of ${required} required approvals of the current head.`,
    )
  } else if (summary && summary.approvals.length > 0) {
    const n = summary.approvals.length
    approved = info(
      'approved',
      'Approved',
      'ok',
      `${n} ${n === 1 ? 'approval' : 'approvals'}`,
      'The approvals the policy requires are in for the current head.',
    )
  } else {
    approved = info(
      'approved',
      'Approved',
      'na',
      'not required',
      'The policy requires no approval for this change.',
    )
  }

  return { mergeable, validated, approved, integrated: integratedInfo }
}

export const STATE_ORDER: readonly StateKey[] = ['mergeable', 'validated', 'approved', 'integrated']

export function statesInOrder(s: FourStates): StateInfo[] {
  return STATE_ORDER.map((k) => s[k])
}
