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
          ? `Check “${check}” has not passed on these exact files`
          : 'A required check has not passed on these exact files',
        hint: 'A trusted runner must report a passing, signed result for these exact files. A new version or a moved branch changes the files, so earlier results do not carry over.',
      }
    }
    case 'failed_validation': {
      const check = str(d.check)
      return {
        ...base,
        title: check
          ? `Check “${check}” failed on these exact files`
          : 'A required check failed on these exact files',
        hint: 'A trusted failure cannot be overridden. Fix the code, update the change request and run the check again.',
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
        hint: 'An eligible reviewer other than the author must approve the latest version. Approvals of an older version no longer count.',
      }
    }
    case 'unresolved_threads': {
      const count = num(d.count)
      return {
        ...base,
        title:
          count === 1
            ? '1 open line discussion must be resolved'
            : `${count ?? 'Some'} open line discussions must be resolved`,
        hint: 'Open Reviews, read all replies and resolve the discussions. Discussions on earlier revisions count too.',
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
        title: 'This agent’s change has no signed record of where it came from',
        hint: 'The agent must attach a signed record (model, reason, checks it ran) for this version.',
      }
    case 'lease': {
      const holder = str(d.holder)
      const scope = str(d.scope)
      const stale = str(d.reason).includes('token')
      return {
        ...base,
        title: `An exclusive reservation${scope ? ` on ${scope}` : ''} is held${holder ? ` by ${holder}` : ''}`,
        hint: stale
          ? 'Land with the reservation holder’s current key, or wait until the reservation is released or expires.'
          : 'Wait for the reservation to be released or to expire.',
      }
    }
    case 'actor_not_allowed':
      return {
        ...base,
        title: 'You are not allowed to land this change under the rules',
        hint: 'Ask someone the rules list as allowed to land changes. Agents can only land changes inside the limits set for them.',
      }
    case 'author_not_allowed':
      return {
        ...base,
        title: 'The author may not propose changes to this branch',
        hint: 'The rules’ allowed_authors does not include the author.',
      }
    case 'too_many_changed_files':
      return {
        ...base,
        title: 'The change touches more files than the rules allow',
        hint: 'Split it into smaller change requests.',
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
