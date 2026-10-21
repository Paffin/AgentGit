// "What needs me": turns what the server says about open proposals and runs into a short, ordered list of things
// a person can act on. Pure (no network, no clock of its own) so every rule is unit-tested.
import type { Preparation, Proposal, Review, Run } from '@/api/types'
import { explainBlocker, hasRefusal } from './blockers'
import { fixesForBlocker, fixesForConflict } from './fixes'
import type { FixId } from './fixes'
import { parseIdentity, samePrincipal } from './identity'
import { summarizeReviews } from './fourstate'
import type { Action } from './roles'
import { DEFAULT_STALE_AFTER_MS, runHealth } from './runs'

export type AttentionKind = 'failed' | 'review' | 'blocked' | 'ready' | 'quiet'

/** Display order, most urgent first. */
export const ATTENTION_ORDER: readonly AttentionKind[] = [
  'failed',
  'review',
  'blocked',
  'ready',
  'quiet',
]

export interface AttentionItem {
  /** Stable across refreshes: `kind:repo:id`. */
  key: string
  kind: AttentionKind
  repo: string
  title: string
  /** Why it is here, in one plain sentence (the first blocker, or what the agent is doing). */
  reason: string
  /** The author or agent, when it matters. */
  who: string
  byAgent: boolean
  /** The change request, when the item is about one. */
  proposalId?: string
  /** What the call to action does (and the query `do=` the proposal page understands). */
  action?: { id: FixId | 'open'; label: string }
  /** More blockers behind the first one. */
  more: number
  ageMs: number
}

export interface RepoFacts {
  repo: string
  proposals: Proposal[]
  /** Keyed by proposal id; a missing entry means "could not be read" and is skipped, never guessed. */
  preps: Record<string, Preparation | undefined>
  reviews: Record<string, readonly Review[] | undefined>
  runs: Run[]
  /** The caller's identity and what they may do in this repository. */
  me: string
  may: (action: Action) => boolean
  now: number
  staleAfterMs?: number
}

const RECENT_FAILURE_MS = 24 * 3600_000

export function attentionForRepo(f: RepoFacts): AttentionItem[] {
  const out: AttentionItem[] = []
  for (const p of f.proposals) {
    if (p.state !== 'open') continue
    const prep = f.preps[p.id]
    if (!prep) continue
    const author = parseIdentity(p.author)
    const isAuthor = samePrincipal(p.author, f.me)
    const ctx = { may: f.may, isAuthor }
    const reviews = f.reviews[p.id]
    const mine = reviews
      ? summarizeReviews(reviews, p.head).current.find((r) => samePrincipal(r.reviewer, f.me))
      : undefined
    const explained = prep.blockers.map(explainBlocker)
    const first = explained[0]
    const base = {
      repo: f.repo,
      title: p.title || '(untitled change request)',
      who: author.name,
      byAgent: author.kind === 'agent',
      proposalId: p.id,
      ageMs: Math.max(0, f.now - p.created_at_ms),
    }
    const needsApproval = prep.blockers.some(
      (b) => b.code === 'insufficient_approvals' || b.code === 'changes_requested',
    )
    if (hasRefusal(prep.blockers)) {
      const b = explained.find((e) => e.severity === 'refused') ?? explained[0]
      out.push({
        ...base,
        key: `failed:${f.repo}:${p.id}`,
        kind: 'failed',
        reason: b ? b.title : 'The rules refuse this change.',
        action: { id: 'checks', label: 'See what failed' },
        more: explained.length - 1,
      })
    } else if (
      needsApproval &&
      !isAuthor &&
      f.may('review') &&
      mine?.decision !== 'approve' &&
      !(mine?.decision === 'request_changes')
    ) {
      const b = explained.find((e) => e.code === 'insufficient_approvals') ?? first
      out.push({
        ...base,
        key: `review:${f.repo}:${p.id}`,
        kind: 'review',
        reason: b ? b.title : 'Waiting for a review.',
        action: { id: 'review', label: 'Review' },
        more: Math.max(0, explained.length - 1),
      })
    } else if (explained.length > 0 || prep.conflicts.length > 0) {
      const conflict = prep.conflicts.length > 0
      const reason = conflict
        ? `Conflicts with the branch in ${prep.conflicts.length === 1 ? 'one file' : `${prep.conflicts.length} files`}`
        : (first?.title ?? 'Blocked')
      const plan = conflict ? fixesForConflict(ctx) : fixesForBlocker(first?.code ?? '', ctx)
      const fix = plan.fixes[0]
      out.push({
        ...base,
        key: `blocked:${f.repo}:${p.id}`,
        kind: 'blocked',
        reason,
        // Opening a page never starts work by itself, so "run the check" is one click further, on the page.
        action:
          fix && fix.id !== 'validate' && fix.id !== 'copy-link'
            ? { id: fix.id, label: fix.label }
            : { id: 'open', label: 'Open' },
        more: Math.max(0, explained.length - (conflict ? 0 : 1)),
      })
    } else if (f.may('integrate')) {
      out.push({
        ...base,
        key: `ready:${f.repo}:${p.id}`,
        kind: 'ready',
        reason: 'Nothing is in the way. You can land it.',
        action: { id: 'land', label: 'Land' },
        more: 0,
      })
    }
  }

  const stale = f.staleAfterMs ?? DEFAULT_STALE_AFTER_MS
  for (const r of f.runs) {
    const h = runHealth(r, f.now, stale)
    if (r.status === 'running' && h.liveness === 'stale') {
      out.push({
        key: `quiet:${f.repo}:${r.run_id}`,
        kind: 'quiet',
        repo: f.repo,
        title: `${parseIdentity(r.identity || `agent:${r.agent}`).name} has gone silent`,
        reason: `No sign of life for ${Math.round(h.ageMs / 60_000)} min. It may be stuck.`,
        who: r.agent,
        byAgent: true,
        action: { id: 'open', label: 'Open agents' },
        more: 0,
        ageMs: h.ageMs,
      })
    } else if (
      r.status === 'failed' &&
      f.now - (r.finished_at_ms || r.started_at_ms) < RECENT_FAILURE_MS
    ) {
      out.push({
        key: `failed:${f.repo}:${r.run_id}`,
        kind: 'failed',
        repo: f.repo,
        title: `${r.agent} stopped with an error`,
        reason: r.summary || 'The session ended as failed.',
        who: r.agent,
        byAgent: true,
        action: { id: 'open', label: 'Open agents' },
        more: 0,
        ageMs: Math.max(0, f.now - (r.finished_at_ms || r.started_at_ms)),
      })
    }
  }
  return out
}

export function groupAttention(
  items: readonly AttentionItem[],
): { kind: AttentionKind; items: AttentionItem[] }[] {
  return ATTENTION_ORDER.map((kind) => ({
    kind,
    items: items.filter((i) => i.kind === kind).sort((a, b) => b.ageMs - a.ageMs),
  })).filter((g) => g.items.length > 0)
}

export const ATTENTION_TITLE: Record<AttentionKind, string> = {
  failed: 'Failures',
  review: 'Waiting for your review',
  blocked: 'Blocked',
  ready: 'Ready to land',
  quiet: 'Agents gone silent',
}

export const ATTENTION_HELP: Record<AttentionKind, string> = {
  failed: 'A check failed or an agent session ended with an error. These need a fix, not a wait.',
  review: 'Someone needs an approval and you are allowed to give it.',
  blocked: 'Something is in the way. Each row says what, and the button does the next step.',
  ready: 'Nothing blocks these. Landing them moves the branch.',
  quiet: 'These agents are running but have not reported in lately.',
}
