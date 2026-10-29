// @vitest-environment node
import { describe, expect, it } from 'vitest'
import type { Blocker, Preparation, Proposal, Review } from '@/api/types'
import { explainAll, explainBlocker, hasRefusal } from './blockers'
import { deriveFourStates, summarizeReviews } from './fourstate'

const blocker = (code: string, detail: Record<string, unknown> = {}, message = code): Blocker => ({
  code,
  message,
  detail,
})

describe('blockers in plain language', () => {
  it('explains a missing validation with the check name and the remedy', () => {
    const e = explainBlocker(
      blocker(
        'missing_validation',
        { check: 'tests' },
        "check 'tests' has no trusted passing attestation",
      ),
    )
    expect(e.title).toContain('“tests”')
    expect(e.hint).toMatch(/signed result/)
    expect(e.severity).toBe('provide')
    expect(e.message).toContain('no trusted passing attestation')
  })

  it('counts missing approvals', () => {
    expect(
      explainBlocker(blocker('insufficient_approvals', { have: 0, required: 2, scope: 'default' }))
        .title,
    ).toBe('Needs 2 more approvals (0 of 2)')
    expect(
      explainBlocker(
        blocker('insufficient_approvals', { have: 1, required: 2, scope: '.github/**' }),
      ).title,
    ).toBe('Needs 1 more approval for .github/** (1 of 2)')
    expect(explainBlocker(blocker('insufficient_approvals')).title).toBe('Not enough approvals')
  })

  it('tells a stale fencing token from a plain lease', () => {
    const stale = explainBlocker(
      blocker('lease', {
        holder: 'agent:bot#run_1',
        scope: 'src/**',
        reason: 'stale_or_missing_token',
      }),
    )
    expect(stale.title).toContain('src/**')
    expect(stale.title).toContain('agent:bot#run_1')
    expect(stale.hint).toMatch(/reservation holder’s current key/)
  })

  it('separates what can be provided from what is refused', () => {
    expect(explainBlocker(blocker('failed_validation', { check: 'lint' })).severity).toBe('refused')
    expect(explainBlocker(blocker('actor_not_allowed')).severity).toBe('refused')
    expect(explainBlocker(blocker('missing_provenance')).severity).toBe('provide')
    expect(hasRefusal([blocker('lease'), blocker('too_many_changed_files')])).toBe(true)
    expect(hasRefusal([blocker('lease')])).toBe(false)
    expect(hasRefusal(undefined)).toBe(false)
  })

  it('falls back to the server wording for codes it does not know', () => {
    const e = explainBlocker(blocker('brand_new', {}, 'something the server said'))
    expect(e.title).toBe('something the server said')
    expect(explainAll(undefined)).toEqual([])
  })
})

const proposal = (over: Partial<Proposal> = {}): Proposal => ({
  id: 'prop_1',
  stream: 'main',
  head: 'h1',
  title: 't',
  description: '',
  author: 'user:alice',
  state: 'open',
  task_id: '',
  created_at_ms: 1,
  updated_at_ms: 1,
  integrated_as: '',
  ...over,
})
const prep = (over: Partial<Preparation> = {}): Preparation => ({
  proposal: 'prop_1',
  strategy: 'fast_forward',
  candidate_tree: 't',
  conflicts: [],
  base: '',
  changed_files: ['a'],
  problem: '',
  algorithm: 'x',
  policy_version: 1,
  blockers: [],
  validations: [],
  jobs: [],
  ...over,
})
const review = (id: number, reviewer: string, decision: string, head = 'h1'): Review => ({
  id,
  proposal: 'prop_1',
  reviewer,
  decision,
  body: '',
  head,
  timestamp_ms: id,
})

describe('the four independent states', () => {
  it('derives each state on its own', () => {
    const s = deriveFourStates({
      proposal: proposal(),
      prep: prep({
        validations: [{ check: 'tests', state: 'missing', attestation_id: '', job_id: '' }],
        blockers: [
          blocker('missing_validation', { check: 'tests' }),
          blocker('insufficient_approvals', { have: 0, required: 1 }),
        ],
      }),
      reviews: [],
    })
    expect(s.mergeable).toMatchObject({ tone: 'ok', value: 'clean' })
    expect(s.validated).toMatchObject({ tone: 'blocked', value: '0/1 passed' })
    expect(s.approved).toMatchObject({ tone: 'blocked', value: '0/1' })
    expect(s.integrated).toMatchObject({ tone: 'pending', value: 'not yet' })
  })

  it('a clean merge says nothing about validation or approval', () => {
    const s = deriveFourStates({
      proposal: proposal(),
      prep: prep({
        strategy: 'merge',
        validations: [{ check: 'tests', state: 'failed', attestation_id: 'a', job_id: '' }],
      }),
      reviews: [review(1, 'user:bob', 'request_changes')],
    })
    expect(s.mergeable.tone).toBe('ok')
    expect(s.validated.tone).toBe('failed')
    expect(s.approved.tone).not.toBe('ok')
  })

  it('reports conflicts with their paths', () => {
    const s = deriveFourStates({
      proposal: proposal(),
      prep: prep({
        strategy: '',
        conflicts: [
          { path: 'src/a.ts', kind: 'content', message: 'content conflict (1 region(s))' },
          { path: 'b', kind: 'both_added', message: 'both sides added different content' },
        ],
      }),
    })
    expect(s.mergeable).toMatchObject({ tone: 'failed', value: '2 conflicts' })
    expect(s.mergeable.summary).toContain('src/a.ts')
  })

  it('shows running validations as pending and passing ones as ok', () => {
    const pending = deriveFourStates({
      proposal: proposal(),
      prep: prep({
        validations: [{ check: 'tests', state: 'pending', attestation_id: '', job_id: 'j' }],
      }),
    })
    expect(pending.validated).toMatchObject({ tone: 'pending', value: 'running' })
    const ok = deriveFourStates({
      proposal: proposal(),
      prep: prep({
        validations: [{ check: 'tests', state: 'passed', attestation_id: 'a', job_id: '' }],
      }),
    })
    expect(ok.validated).toMatchObject({ tone: 'ok', value: '1/1 passed' })
  })

  it('no required checks is "none required", not green', () => {
    expect(deriveFourStates({ proposal: proposal(), prep: prep() }).validated).toMatchObject({
      tone: 'na',
      value: 'none required',
    })
  })

  it('counts approvals of the current head and flags changes requested', () => {
    const ok = deriveFourStates({
      proposal: proposal(),
      prep: prep(),
      reviews: [review(1, 'user:bob', 'approve')],
    })
    expect(ok.approved).toMatchObject({ tone: 'ok', value: '1 approval' })
    const changes = deriveFourStates({
      proposal: proposal(),
      prep: prep({ blockers: [blocker('changes_requested')] }),
      reviews: [review(1, 'user:bob', 'request_changes')],
    })
    expect(changes.approved).toMatchObject({ tone: 'failed', value: 'changes requested' })
  })

  it('shows integrated and closed proposals without pretending to recompute', () => {
    const done = deriveFourStates({
      proposal: proposal({ state: 'integrated', integrated_as: 'h1' }),
    })
    expect(done.integrated.tone).toBe('ok')
    expect(done.validated.value).toBe('at integration')
    const closed = deriveFourStates({ proposal: proposal({ state: 'closed' }) })
    expect(closed.integrated).toMatchObject({ tone: 'na', value: 'closed' })
    expect(closed.mergeable.tone).toBe('na')
  })

  it('is honest while loading', () => {
    const s = deriveFourStates({ proposal: proposal(), prep: null, reviews: null })
    expect(s.mergeable.tone).toBe('unknown')
    expect(s.validated.tone).toBe('unknown')
    expect(s.approved.tone).toBe('unknown')
  })

  it('does not call approval "not required" without the policy check', () => {
    // prepare is loading or failed, the reviews are known and empty: nothing proves the policy asks for no approval
    const s = deriveFourStates({ proposal: proposal(), prep: null, reviews: [] })
    expect(s.approved).toMatchObject({ tone: 'unknown', value: 'checking' })
    // with the check done and nothing required, it is "not required"
    expect(
      deriveFourStates({ proposal: proposal(), prep: prep(), reviews: [] }).approved,
    ).toMatchObject({
      tone: 'na',
    })
  })

  it('does not invent the number of required approvals', () => {
    const s = deriveFourStates({
      proposal: proposal(),
      prep: prep({ blockers: [blocker('insufficient_approvals', { have: 1 })] }),
      reviews: [],
    })
    expect(s.approved.tone).toBe('blocked')
    expect(s.approved.value).toBe('1 approval')
    expect(s.approved.value).not.toContain('/')
  })

  it('surfaces a problem string from the merge engine', () => {
    expect(
      deriveFourStates({
        proposal: proposal(),
        prep: prep({ problem: 'criss-cross merge', strategy: '' }),
      }).mergeable,
    ).toMatchObject({
      tone: 'failed',
      summary: 'criss-cross merge',
    })
  })
})

describe('review summaries', () => {
  it('takes the latest decision per reviewer principal and separates stale approvals', () => {
    const s = summarizeReviews(
      [
        review(1, 'user:bob', 'approve'),
        review(2, 'user:bob', 'request_changes'),
        review(3, 'user:carol', 'approve', 'old-head'),
        review(4, 'agent:rev#run_1', 'approve'),
        review(5, 'agent:rev#run_2', 'comment'),
        review(6, 'user:bob', 'comment'),
      ],
      'h1',
    )
    expect(s.approvals.map((r) => r.reviewer)).toEqual(['agent:rev#run_1'])
    expect(s.changesRequested.map((r) => r.reviewer)).toEqual(['user:bob'])
    expect(s.staleApprovals.map((r) => r.reviewer)).toEqual(['user:carol'])
    // A later discussion comment does not revoke a review decision.
    expect(s.current.filter((r) => r.reviewer.startsWith('agent:rev'))).toHaveLength(1)
  })
})

it('explains required discussions without calling them an approval or refusal', () => {
  const explanation = explainBlocker(blocker('unresolved_threads', { count: 2 }))
  expect(explanation.title).toBe('2 open line discussions must be resolved')
  expect(explanation.hint).toContain('earlier revisions')
  expect(explanation.severity).toBe('provide')
})
