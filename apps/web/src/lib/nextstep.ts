// One sentence that says what to do FIRST on a change request. The blocker list says everything that is in the
// way; this picks the one to start with, in the order that avoids wasted work (a conflict changes the files, so
// checks and approvals made before it would not carry over).
import type { Blocker, Conflict } from '@/api/types'
import type { Action } from './roles'

export interface NextContext {
  may: (action: Action) => boolean
  isAuthor: boolean
}

export function nextStep(
  blockers: readonly Blocker[],
  conflicts: readonly Conflict[],
  ctx: NextContext,
): string {
  const has = (code: string) => blockers.some((b) => b.code === code)
  if (conflicts.length > 0)
    return ctx.isAuthor
      ? 'Resolve the conflict with the branch.'
      : 'The author needs to resolve the conflict with the branch.'
  if (has('failed_validation'))
    return ctx.isAuthor
      ? 'Fix the failed check, update the change request, then run the checks again.'
      : 'The author needs to fix the failed check.'
  if (has('missing_validation'))
    return ctx.may('requestValidation')
      ? 'Run the checks.'
      : 'Ask someone with the writer or integrator role to run the checks.'
  if (has('missing_provenance'))
    return 'The agent has to attach its signed record of where this came from.'
  if (has('unresolved_threads')) return 'Read and resolve the open line discussions in Reviews.'
  const need = blockers.find((b) => b.code === 'insufficient_approvals')
  if (need || has('changes_requested')) {
    const d = (need?.detail ?? {}) as { have?: number; required?: number }
    const missing =
      typeof d.have === 'number' && typeof d.required === 'number'
        ? Math.max(1, d.required - d.have)
        : 1
    const what = has('changes_requested')
      ? 'The reviewer who asked for changes has to approve again.'
      : `Get ${missing} more ${missing === 1 ? 'approval' : 'approvals'} from a reviewer.`
    return ctx.may('review') && !ctx.isAuthor
      ? `${what} You can review it.`
      : `${what} ${ctx.isAuthor ? 'You cannot approve your own change.' : 'Ask someone with the reviewer role.'}`
  }
  if (has('lease')) return 'Land it with the reservation key, or wait for the reservation to end.'
  if (blockers.length > 0) return 'Read what is in the way below.'
  return ctx.may('integrate') ? 'Land it.' : 'Ask someone with the integrator role to land it.'
}
