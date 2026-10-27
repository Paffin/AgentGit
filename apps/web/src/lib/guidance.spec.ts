import { describe, expect, it } from 'vitest'
import type { Blocker, Preparation, Proposal, Review, Run } from '@/api/types'
import { attentionForRepo, groupAttention } from './attention'
import { fixesForBlocker, fixesForConflict } from './fixes'
import { GLOSSARY, TERM_IDS, glossaryTable, termLabel, termTip } from './glossary'
import { mayDo, parseRoleGrants, rolesForRepo } from './roles'
import type { Action } from './roles'

const NOW = 1_800_000_000_000
const blocker = (code: string, detail: Record<string, unknown> = {}): Blocker => ({
  code,
  message: code,
  detail,
})
const prep = (blockers: Blocker[] = [], conflicts: Preparation['conflicts'] = []): Preparation =>
  ({
    blockers,
    conflicts,
    jobs: [],
    validations: [],
    changed_files: [],
    strategy: 'merge',
    problem: '',
    policy_version: 1,
  }) as unknown as Preparation
const proposal = (id: string, over: Partial<Proposal> = {}): Proposal => ({
  id,
  stream: 'main',
  head: `h-${id}`,
  title: `Title ${id}`,
  description: '',
  author: 'agent:bot#run_1',
  state: 'open',
  task_id: '',
  created_at_ms: NOW - 120_000,
  updated_at_ms: NOW,
  integrated_as: '',
  ...over,
})
const review = (p: Proposal, who: string, decision: string, id = 1): Review => ({
  id,
  proposal: p.id,
  reviewer: who,
  decision,
  head: p.head,
  body: '',
  timestamp_ms: NOW,
})
const run = (over: Partial<Run>): Run => ({
  run_id: 'run_x',
  agent: 'bot',
  identity: 'agent:bot#run_x',
  status: 'running',
  started_at_ms: NOW - 3_600_000,
  last_heartbeat_ms: NOW - 1000,
  finished_at_ms: 0,
  summary: '',
  task_id: '',
  ...over,
})
const mayOf = (roles: string[]) => {
  const have = rolesForRepo(parseRoleGrants(roles), 'acme/app')
  return (a: Action) => mayDo(have, a)
}

describe('glossary', () => {
  it('has a label, the AgentGit word and a tip for every term, and no term is empty', () => {
    expect(TERM_IDS.length).toBeGreaterThanOrEqual(15)
    for (const id of TERM_IDS) {
      const e = GLOSSARY[id]
      expect(e.label.length, id).toBeGreaterThan(1)
      expect(e.word.length, id).toBeGreaterThan(1)
      expect(e.tip.endsWith('.') || e.tip.endsWith('?'), id).toBe(true)
    }
  })

  it('covers every word the first outside review stumbled on', () => {
    for (const id of [
      'proposal',
      'attestation',
      'provenance',
      'lease',
      'fencing',
      'stream',
      'verdict',
    ] as const)
      expect(TERM_IDS).toContain(id)
  })

  it('shows the plain label and keeps the AgentGit word in the tip', () => {
    expect(termLabel('lease')).toBe('Reservation')
    expect(termLabel('lease', true)).toBe('Reservations')
    expect(termLabel('stream', true)).toBe('Branches')
    expect(termTip('fencing')).toContain('“fencing token”')
  })

  it('does not use two terms with the same plain label', () => {
    const labels = TERM_IDS.map((id) => GLOSSARY[id].label)
    expect(new Set(labels).size).toBe(labels.length)
  })

  it('prints a table with one row per term', () => {
    expect(glossaryTable().split('\n')).toHaveLength(TERM_IDS.length + 2)
  })
})

describe('fixes: only what the role may do', () => {
  const ctx = (roles: string[], isAuthor = false) => ({ may: mayOf(roles), isAuthor })

  it('a missing check is fixed by running it, by a writer', () => {
    expect(fixesForBlocker('missing_validation', ctx(['acme/*:writer'])).fixes[0]?.id).toBe(
      'validate',
    )
  })

  it('a reviewer cannot run checks: no button, and the text names who can', () => {
    const plan = fixesForBlocker('missing_validation', ctx(['acme/*:reviewer']))
    expect(plan.fixes).toEqual([])
    expect(plan.ask).toMatch(/writer/)
  })

  it('missing approval: a reviewer reviews, anyone can copy a link, an author may not approve', () => {
    const rev = fixesForBlocker('insufficient_approvals', ctx(['acme/*:reviewer']))
    expect(rev.fixes.map((f) => f.id)).toEqual(['review', 'copy-link'])
    const author = fixesForBlocker('insufficient_approvals', ctx(['acme/*:reviewer'], true))
    expect(author.fixes.map((f) => f.id)).toEqual(['copy-link'])
    expect(author.ask).toMatch(/own change/)
    const writer = fixesForBlocker('insufficient_approvals', ctx(['acme/*:writer']))
    expect(writer.fixes.map((f) => f.id)).toEqual(['copy-link'])
    expect(writer.ask).toMatch(/reviewer/)
  })

  it('a reservation is fixed by landing with its key, only for someone who may land', () => {
    expect(fixesForBlocker('lease', ctx(['acme/*:integrator'])).fixes[0]?.id).toBe('land')
    expect(fixesForBlocker('lease', ctx(['acme/*:reader'])).fixes).toEqual([])
  })

  it('a failed check has no override: it points at the failure', () => {
    const plan = fixesForBlocker('failed_validation', ctx(['acme/*:admin']))
    expect(plan.fixes.map((f) => f.id)).toEqual(['checks'])
  })

  it('an unknown code offers nothing rather than guessing', () => {
    expect(fixesForBlocker('something_new', ctx(['acme/*:admin']))).toEqual({ fixes: [] })
  })

  it('a conflict is resolved by its author in the browser; others pass the link on', () => {
    expect(fixesForConflict(ctx(['acme/*:writer'], true)).fixes[0]?.id).toBe('conflict')
    expect(fixesForConflict(ctx(['acme/*:writer'], false)).fixes[0]?.id).toBe('copy-link')
    expect(fixesForConflict(ctx(['acme/*:reader'], true)).fixes[0]?.id).toBe('copy-link')
  })
})

describe('attention: what needs me', () => {
  const base = { repo: 'acme/app', me: 'user:bob', now: NOW }

  it('sorts a mixed repository into the five kinds, with the reason and the action', () => {
    const ps = [
      proposal('failed'),
      proposal('review'),
      proposal('lease'),
      proposal('clear'),
      proposal('conflict'),
    ]
    const items = attentionForRepo({
      ...base,
      may: mayOf(['acme/*:integrator', 'acme/*:reviewer']),
      proposals: ps,
      preps: {
        failed: prep([blocker('failed_validation', { check: 'tests' })]),
        review: prep([blocker('insufficient_approvals', { have: 0, required: 1 })]),
        lease: prep([blocker('lease', { holder: 'agent:x', scope: 'src/**' })]),
        clear: prep(),
        conflict: prep([], [{ path: 'a.txt', kind: 'content', message: '' }]),
      },
      reviews: {},
      runs: [
        run({ run_id: 'r1', last_heartbeat_ms: NOW - 600_000 }),
        run({ run_id: 'r2', status: 'failed', finished_at_ms: NOW - 1000, summary: 'oom' }),
        run({ run_id: 'r3', status: 'completed' }),
      ],
    })
    const kinds = Object.fromEntries(items.map((i) => [i.key.split(':')[2], i.kind]))
    expect(kinds).toMatchObject({
      failed: 'failed',
      review: 'review',
      lease: 'blocked',
      clear: 'ready',
      conflict: 'blocked',
      r1: 'quiet',
      r2: 'failed',
    })
    expect(kinds.r3).toBeUndefined()
    const lease = items.find((i) => i.key.endsWith(':lease'))
    expect(lease?.action).toEqual({ id: 'land', label: 'Land with the reservation key' })
    expect(lease?.reason).toMatch(/reservation|lease/i)
    const conflict = items.find((i) => i.key.endsWith(':conflict'))
    expect(conflict?.reason).toMatch(/Conflicts with the branch in one file/)
    expect(groupAttention(items).map((g) => g.kind)).toEqual([
      'failed',
      'review',
      'blocked',
      'ready',
      'quiet',
    ])
  })

  it('a review is only asked of someone who may review, is not the author, and has not decided', () => {
    const p = proposal('p', { author: 'user:alice' })
    const needs = prep([blocker('insufficient_approvals', { have: 0, required: 1 })])
    const call = (me: string, roles: string[], reviews: Review[] = []) =>
      attentionForRepo({
        ...base,
        me,
        may: mayOf(roles),
        proposals: [p],
        preps: { p: needs },
        reviews: { p: reviews },
        runs: [],
      })[0]
    expect(call('user:bob', ['acme/*:reviewer'])?.kind).toBe('review')
    expect(call('user:alice', ['acme/*:reviewer'])?.kind).toBe('blocked')
    expect(call('user:erin', ['acme/*:reader'])?.kind).toBe('blocked')
    expect(call('user:bob', ['acme/*:reviewer'], [review(p, 'user:bob', 'approve')])?.kind).toBe(
      'blocked',
    )
  })

  it('says "ready to land" only to someone who may land', () => {
    const args = {
      ...base,
      proposals: [proposal('p')],
      preps: { p: prep() },
      reviews: {},
      runs: [],
    }
    expect(attentionForRepo({ ...args, may: mayOf(['acme/*:reader']) })).toEqual([])
    expect(attentionForRepo({ ...args, may: mayOf(['acme/*:integrator']) })[0]?.kind).toBe('ready')
  })

  it('skips a proposal whose preparation could not be read, and closed ones; never guesses', () => {
    const items = attentionForRepo({
      ...base,
      may: mayOf(['acme/*:admin']),
      proposals: [proposal('a'), proposal('b', { state: 'integrated' })],
      preps: { a: undefined, b: prep() },
      reviews: {},
      runs: [],
    })
    expect(items).toEqual([])
  })

  it('counts the blockers behind the first one', () => {
    const [item] = attentionForRepo({
      ...base,
      may: mayOf(['acme/*:writer']),
      proposals: [proposal('p')],
      preps: {
        p: prep([blocker('missing_validation', { check: 'tests' }), blocker('lease')]),
      },
      reviews: {},
      runs: [],
    })
    expect(item?.more).toBe(1)
    // starting work is never done by merely arriving on a page: the home row opens it, the page has the button
    expect(item?.action?.id).toBe('open')
  })

  it('an old failed session is not news', () => {
    const items = attentionForRepo({
      ...base,
      may: mayOf(['acme/*:admin']),
      proposals: [],
      preps: {},
      reviews: {},
      runs: [run({ status: 'failed', finished_at_ms: NOW - 3 * 86_400_000 })],
    })
    expect(items).toEqual([])
  })
})
