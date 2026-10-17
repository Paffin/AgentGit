// Turns the engine's stable blocker codes (docs/guides/policy.md#outcome-codes) into plain language with a
// remedy. The server's own message is always kept: it is the exact statement of what is wrong.
import type { Blocker } from '@/api/types'

export interface Explained {
  code: string
  /** One short sentence. */
  title: string
  /** What can be done about it. */
  hint: string
  /** `refused`: no amount of waiting fixes this (exit code 4). `provide`: supply it and run again (exit code 3). */
  severity: 'provide' | 'refused'
  /** The server's exact wording. */
  message: string
}

const REFUSED = new Set([
  'failed_validation',
  'actor_not_allowed',
  'author_not_allowed',
  'too_many_changed_files',
])

function detailOf(b: Blocker): Record<string, unknown> {
  return b.detail && typeof b.detail === 'object' ? (b.detail as Record<string, unknown>) : {}
}

const str = (v: unknown): string =>
  typeof v === 'string' ? v : v === undefined || v === null ? '' : String(v)
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)

export function explainBlocker(b: Blocker): Explained {
  const d = detailOf(b)
  const base = {
    code: b.code,
    message: b.message,
    severity: (REFUSED.has(b.code) ? 'refused' : 'provide') as Explained['severity'],
  }
  switch (b.code) {
    case 'missing_validation': {
      const check = str(d.check)
      return {
        ...base,
        title: check
          ? `Check “${check}” has not passed on this exact tree`
          : 'A required check has not passed on this exact tree',
        hint: 'A trusted runner must submit a passing, signed attestation for this tree. A new commit or a moved stream changes the tree, so earlier results do not carry over.',
      }
    }
    case 'failed_validation': {
      const check = str(d.check)
      return {
        ...base,
        title: check
          ? `Check “${check}” failed on this exact tree`
          : 'A required check failed on this exact tree',
        hint: 'A trusted failure cannot be overridden. Fix the code, update the proposal and validate again.',
      }
    }
    case 'insufficient_approvals': {
      const have = num(d.have)
      const required = num(d.required)
      const scope = str(d.scope)
      const missing = have !== null && required !== null ? Math.max(0, required - have) : null
      const where = scope && scope !== 'default' ? ` for ${scope}` : ''
      const title =
        missing !== null && have !== null && required !== null
          ? `Needs ${missing} more ${missing === 1 ? 'approval' : 'approvals'}${where} (${have} of ${required})`
          : 'Not enough approvals'
      return {
        ...base,
        title,
        hint: 'An eligible reviewer other than the author must approve the current head. Approvals of an older head no longer count.',
      }
    }
    case 'changes_requested':
      return {
        ...base,
        title: 'A reviewer asked for changes',
        hint: 'The change stays blocked until that reviewer approves again, after the requested changes.',
      }
    case 'missing_provenance':
      return {
        ...base,
        title: 'This agent-authored change has no signed provenance',
        hint: 'The agent must attach a signed provenance record (model, rationale, checks run) for this head.',
      }
    case 'lease': {
      const holder = str(d.holder)
      const scope = str(d.scope)
      const stale = str(d.reason).includes('token')
      return {
        ...base,
        title: `An exclusive lease${scope ? ` on ${scope}` : ''} is held${holder ? ` by ${holder}` : ''}`,
        hint: stale
          ? 'Integrate with the lease holder’s current fencing token, or wait until the lease is released or expires.'
          : 'Wait for the lease to be released or to expire.',
      }
    }
    case 'actor_not_allowed':
      return {
        ...base,
        title: 'You are not allowed to integrate this change under the policy',
        hint: 'Ask someone the policy lists as an integrator. Agents can only integrate inside the autonomy envelope.',
      }
    case 'author_not_allowed':
      return {
        ...base,
        title: 'The author may not propose to this stream',
        hint: 'The policy’s allowed_authors does not include the author.',
      }
    case 'too_many_changed_files':
      return {
        ...base,
        title: 'The change touches more files than the policy allows',
        hint: 'Split it into smaller proposals.',
      }
    default:
      return { ...base, title: b.message || b.code, hint: '' }
  }
}

export function explainAll(blockers: readonly Blocker[] | undefined): Explained[] {
  return (blockers ?? []).map(explainBlocker)
}

export function hasRefusal(blockers: readonly Blocker[] | undefined): boolean {
  return (blockers ?? []).some((b) => REFUSED.has(b.code))
}
