// What a person can DO about a blocker, as actions the screen can offer. A blocker explains itself in
// lib/blockers.ts; this file says which button fixes it. Only actions the caller's role may do are offered as
// buttons (the server still decides); when the role is not enough the answer is a sentence naming who to ask.
import type { Action } from './roles'
import { requirementText } from './roles'

export type FixId =
  'validate' | 'review' | 'copy-link' | 'land' | 'conflict' | 'checks' | 'reviews' | 'provenance'

export interface Fix {
  id: FixId
  label: string
}

export interface FixPlan {
  /** Buttons the caller may press. */
  fixes: Fix[]
  /** Something to say instead when no button applies ("Ask someone with the reviewer role"). */
  ask?: string
}

export interface FixContext {
  may: (action: Action) => boolean
  /** The caller wrote this change (an author cannot approve their own work). */
  isAuthor: boolean
}

const askFor = (action: Action): string =>
  `${requirementText(action).replace('Requires', 'Ask someone with')}.`

/** Fixes for one blocker code (the codes are the engine's, see docs/guides/policy.md#outcome-codes). */
export function fixesForBlocker(code: string, ctx: FixContext): FixPlan {
  switch (code) {
    case 'missing_validation':
      return ctx.may('requestValidation')
        ? { fixes: [{ id: 'validate', label: 'Run the check' }] }
        : { fixes: [], ask: askFor('requestValidation') }
    case 'failed_validation':
      return {
        fixes: [{ id: 'checks', label: 'See what failed' }],
        ask: ctx.isAuthor
          ? 'Fix the code, update the change request, then run the check again.'
          : 'The author needs to fix the code and update the change request.',
      }
    case 'insufficient_approvals':
    case 'changes_requested': {
      const fixes: Fix[] = []
      if (ctx.may('review') && !ctx.isAuthor) {
        fixes.push({ id: 'review', label: 'Review this change' })
      }
      fixes.push({ id: 'copy-link', label: 'Copy link to ask a reviewer' })
      if (code === 'changes_requested') fixes.push({ id: 'reviews', label: 'Read the review' })
      return ctx.may('review') && !ctx.isAuthor
        ? { fixes }
        : { fixes, ask: ctx.isAuthor ? 'You cannot approve your own change.' : askFor('review') }
    }
    case 'missing_provenance':
      return {
        fixes: [{ id: 'provenance', label: 'See where it came from' }],
        ask: 'The agent must attach its signed record for this version.',
      }
    case 'lease':
      return ctx.may('integrate')
        ? { fixes: [{ id: 'land', label: 'Land with the reservation key' }] }
        : { fixes: [], ask: askFor('integrate') }
    case 'actor_not_allowed':
      return { fixes: [], ask: 'Ask someone the rules list as allowed to land changes.' }
    default:
      return { fixes: [] }
  }
}

/** Fixes for a conflict with the branch: the author resolves it in the browser, others ask the author. */
export function fixesForConflict(ctx: FixContext): FixPlan {
  return ctx.isAuthor && ctx.may('editFiles')
    ? { fixes: [{ id: 'conflict', label: 'Resolve the conflict' }] }
    : { fixes: [{ id: 'copy-link', label: 'Copy link to send to the author' }] }
}
