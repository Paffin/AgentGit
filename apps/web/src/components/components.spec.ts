import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { Blocker, Lease, Proposal } from '@/api/types'
import { deriveFourStates } from '@/lib/fourstate'
import { describeEvent } from '@/lib/events'
import { buildGraph } from '@/lib/graph'
import BlockerList from './BlockerList.vue'
import ProposalCard from './ProposalCard.vue'
import StateStrip from './StateStrip.vue'
import ActivityRow from './ActivityRow.vue'
import GraphCell from './history/GraphCell.vue'
import ValidationTab from './proposal/ValidationTab.vue'
import ProvenanceTab from './proposal/ProvenanceTab.vue'
import ReviewsTab from './proposal/ReviewsTab.vue'
import WhoamiCard from './WhoamiCard.vue'

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:p(.*)*', component: { template: '<i/>' } }],
})
const global = { plugins: [router] }

const EVIL = '<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>'

const proposal = (over: Partial<Proposal> = {}): Proposal => ({
  id: 'prop_8d079ff9771014b419e9',
  stream: 'main',
  head: 'h1',
  title: 'Extract session store',
  description: '',
  author: 'agent:refactor-bot#run_978fd205c98d',
  state: 'open',
  task_id: '',
  created_at_ms: Date.now() - 60_000,
  updated_at_ms: Date.now(),
  integrated_as: '',
  ...over,
})
const blocker = (code: string, detail: Record<string, unknown> = {}): Blocker => ({
  code,
  message: `server says ${code}`,
  detail,
})

describe('BlockerList', () => {
  it('lists blockers in plain language, with the server wording on hover and remedies on request', () => {
    const w = mount(BlockerList, {
      props: {
        blockers: [
          blocker('missing_validation', { check: 'tests' }),
          blocker('failed_validation', { check: 'lint' }),
        ],
        hints: true,
      },
    })
    expect(w.findAll('li')).toHaveLength(2)
    expect(w.find('li .title').attributes('title')).toBe('server says missing_validation')
    expect(w.text()).toContain('trusted runner')
    expect(w.find('[data-code="failed_validation"]').classes()).toContain('refused')
    expect(w.find('[data-code="missing_validation"]').classes()).toContain('provide')
  })
  it('collapses the tail', () => {
    const w = mount(BlockerList, {
      props: { blockers: ['a', 'b', 'c', 'd', 'e'].map((c) => blocker(c)), max: 3 },
    })
    expect(w.text()).toContain('and 2 more')
  })
  it('renders nothing for no blockers', () => {
    expect(
      mount(BlockerList, { props: { blockers: [] } })
        .find('ul')
        .exists(),
    ).toBe(false)
  })
})

describe('StateStrip', () => {
  it('shows exactly four states, in order, as separate badges', () => {
    const p = proposal()
    const w = mount(StateStrip, { props: { states: deriveFourStates({ proposal: p }) } })
    expect(w.findAll('li')).toHaveLength(4)
    expect(w.findAll('[data-state]').map((e) => e.attributes('data-state'))).toEqual([
      'mergeable',
      'validated',
      'approved',
      'integrated',
    ])
  })
})

describe('ProposalCard', () => {
  it('shows author, four states and the exact blockers', () => {
    const p = proposal()
    const prep = {
      data: {
        proposal: p.id,
        strategy: 'fast_forward',
        candidate_tree: 't',
        conflicts: [],
        base: '',
        changed_files: ['a', 'b'],
        problem: '',
        algorithm: '',
        policy_version: 1,
        blockers: [blocker('insufficient_approvals', { have: 0, required: 1 })],
        validations: [{ check: 'tests', state: 'passed', attestation_id: 'a', job_id: '' }],
        jobs: [],
      },
    }
    const w = mount(ProposalCard, {
      props: {
        repo: 'acme/web',
        proposal: p,
        prep,
        states: deriveFourStates({ proposal: p, prep: prep.data, reviews: [] }),
      },
      global,
    })
    expect(w.find('a').attributes('href')).toBe(`/repos/acme/web/proposals/${p.id}`)
    expect(w.text()).toContain('refactor-bot')
    expect(w.findAll('[data-state]')).toHaveLength(4)
    expect(w.find('[data-state="validated"]').attributes('data-tone')).toBe('ok')
    expect(w.find('[data-state="approved"]').attributes('data-tone')).toBe('blocked')
    expect(w.find('[data-code="insufficient_approvals"]').text()).toContain('Needs 1 more approval')
    expect(w.text()).toContain('2 files')
  })

  it('lists conflicting paths in plain sight', () => {
    const p = proposal()
    const prep = {
      data: {
        proposal: p.id,
        strategy: '',
        candidate_tree: '',
        conflicts: [
          { path: 'src/index.ts', kind: 'content', message: 'content conflict (2 region(s))' },
        ],
        base: 'b',
        changed_files: [],
        problem: '',
        algorithm: '',
        policy_version: 1,
        blockers: [],
        validations: [],
        jobs: [],
      },
    }
    const w = mount(ProposalCard, {
      props: {
        repo: 'a/b',
        proposal: p,
        prep,
        states: deriveFourStates({ proposal: p, prep: prep.data }),
      },
      global,
    })
    expect(w.text()).toContain('src/index.ts')
    // the words for a person, not the code a program switches on
    expect(w.text()).toContain('content conflict (2 region(s))')
    expect(w.text()).toContain('Conflicting paths')
    expect(w.find('[data-state="mergeable"]').attributes('data-tone')).toBe('failed')
  })

  it('words a directory rename conflict even when the server sent no message', () => {
    const p = proposal()
    const prep = {
      data: {
        proposal: p.id,
        strategy: '',
        candidate_tree: '',
        conflicts: [{ path: 'lib/new.ts', kind: 'directory_rename_collision', message: '' }],
        base: 'b',
        changed_files: [],
        problem: '',
        algorithm: '',
        policy_version: 1,
        blockers: [],
        validations: [],
        jobs: [],
      },
    }
    const w = mount(ProposalCard, {
      props: {
        repo: 'a/b',
        proposal: p,
        prep,
        states: deriveFourStates({ proposal: p, prep: prep.data }),
      },
      global,
    })
    expect(w.text()).toContain('lib/new.ts')
    expect(w.text()).toContain(
      'directory rename: a moved file lands on a path that is already taken',
    )
  })

  it('treats title, author and conflicts from the server as text only', () => {
    const p = proposal({ title: EVIL, author: `user:${EVIL}` })
    const prep = {
      data: {
        proposal: p.id,
        strategy: '',
        candidate_tree: '',
        conflicts: [{ path: EVIL, kind: EVIL, message: EVIL }],
        base: '',
        changed_files: [],
        problem: '',
        algorithm: '',
        policy_version: 1,
        blockers: [{ code: EVIL, message: EVIL, detail: {} }],
        validations: [],
        jobs: [],
      },
    }
    const w = mount(ProposalCard, {
      props: {
        repo: 'a/b',
        proposal: p,
        prep,
        states: deriveFourStates({ proposal: p, prep: prep.data }),
      },
      global,
    })
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('script').exists()).toBe(false)
    expect(w.text()).toContain('<img src=x')
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined()
  })
})

describe('ActivityRow', () => {
  it('turns an event into a sentence and links proposals', () => {
    const ev = describeEvent({
      seq: 5,
      topic: 'proposal.review',
      repo: 'a/b',
      timestamp_ms: Date.now(),
      payload: {
        actor: 'user:bob',
        target: 'prop_8d079ff9771014b419e9',
        details: { decision: 'approve' },
      },
    })
    const w = mount(ActivityRow, { props: { event: ev, repo: 'acme/web' }, global })
    expect(w.text()).toContain('bob')
    expect(w.text()).toContain('approved')
    expect(w.find('a').attributes('href')).toBe(
      '/repos/acme/web/proposals/prop_8d079ff9771014b419e9',
    )
    expect(w.attributes('data-topic')).toBe('proposal.review')
  })
  it('marks fresh events and dims housekeeping', () => {
    const minor = describeEvent({
      seq: 1,
      topic: 'workspace.modify',
      repo: '',
      timestamp_ms: 1,
      payload: { target: 'w' },
    })
    expect(
      mount(ActivityRow, { props: { event: minor, repo: 'a/b', fresh: true }, global }).classes(),
    ).toEqual(expect.arrayContaining(['fresh', 'minor']))
  })
  it('renders hostile payloads as text', () => {
    const ev = describeEvent({
      seq: 1,
      topic: 'lease.acquire',
      repo: '',
      timestamp_ms: 1,
      payload: { actor: `agent:${EVIL}`, target: EVIL, details: { scope: EVIL } },
    })
    const w = mount(ActivityRow, { props: { event: ev, repo: 'a/b' }, global })
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('script').exists()).toBe(false)
  })
})

describe('GraphCell', () => {
  it('draws the dot, the lines through the row and the merge ring', () => {
    const { rows } = buildGraph([
      { id: 'm', parents: ['b', 's'] },
      { id: 'b', parents: ['a'] },
    ])
    const merge = mount(GraphCell, { props: { row: rows[0]!, lanes: 2 } })
    expect(merge.attributes('aria-hidden')).toBe('true')
    expect(merge.findAll('circle')).toHaveLength(1)
    expect(merge.find('circle').attributes('fill')).toBe('var(--surface)') // hollow = merge
    expect(merge.findAll('path').length).toBe(2) // one line per parent
    const normal = mount(GraphCell, { props: { row: rows[1]!, lanes: 2 } })
    expect(normal.find('circle').attributes('fill')).not.toBe('var(--surface)')
    // the second lane passes straight through the second row
    expect(normal.findAll('path').length).toBe(3)
  })
})

describe('WhoamiCard', () => {
  it('shows identity, method, expiry, roles and scope', () => {
    const w = mount(WhoamiCard, {
      props: {
        principal: {
          identity: 'agent:refactor-bot',
          kind: 'agent',
          method: 'token',
          credential_id: 'cred_1',
          expires_at_ms: Date.now() + 3_600_000,
          roles: ['acme/*:writer'],
          scope: { repos: ['acme/web'], streams: ['agents/*'], operations: [], run_id: '' },
        },
      },
    })
    expect(w.text()).toContain('refactor-bot')
    expect(w.text()).toContain('Scoped token')
    expect(w.text()).toContain('acme/*:writer')
    expect(w.text()).toContain('acme/web')
    expect(w.text()).toContain('agents/*')
    expect(w.text()).toMatch(/\(\d+h \d+m\)|\(59m/)
  })
  it('does not promise public content to a principal without roles', () => {
    const w = mount(WhoamiCard, {
      props: {
        principal: {
          identity: 'user:nobody',
          kind: 'user',
          method: 'token',
          credential_id: 'cred_2',
          expires_at_ms: 0,
          roles: [],
          scope: { repos: [], streams: [], operations: [], run_id: '' },
        },
      },
    })
    expect(w.text()).toContain('cannot see any repository')
    expect(w.text()).not.toContain('public')
  })
  it('says plainly when nothing limits a credential or it never expires', () => {
    const w = mount(WhoamiCard, {
      props: {
        principal: {
          identity: 'user:root',
          kind: 'user',
          method: 'bootstrap',
          credential_id: 'bootstrap',
          expires_at_ms: 0,
          roles: ['*:admin'],
          scope: { repos: [], streams: [], operations: [], run_id: '' },
        },
      },
    })
    expect(w.text()).toContain('never')
    expect(w.text()).toContain('unrestricted by scope')
    expect(w.text()).toContain('Bootstrap')
  })
})

describe('proposal tabs', () => {
  it('provenance: marks verified signatures and shows agent text as untrusted', () => {
    const entry = {
      id: 1,
      record: {
        changeset: 'c'.repeat(64),
        agent: 'agent:bot#run_1',
        run_id: 'run_1',
        task_id: '',
        model: 'example-model',
        prompt_digest: '',
        parameters: { temperature: '0.2' },
        checks_run: ['npm test'],
        inputs: [EVIL],
        rationale: EVIL,
        created_at_ms: 1,
      },
      public_key: 'k'.repeat(64),
      signature: 's',
      verified: true,
      created_at_ms: 1,
    }
    const w = mount(ProvenanceTab, { props: { entries: [entry], authorIsAgent: true } })
    expect(w.text()).toContain('signature verified')
    expect(w.text()).toContain('example-model')
    expect(w.text()).toContain('untrusted text')
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('script').exists()).toBe(false)
    expect(w.find('blockquote').text()).toBe(EVIL)
    const unverified = mount(ProvenanceTab, {
      props: { entries: [{ ...entry, verified: false }], authorIsAgent: true },
    })
    expect(unverified.text()).toContain('signature not verified')
  })
  it('provenance: explains the absence for agent authors', () => {
    expect(mount(ProvenanceTab, { props: { entries: [], authorIsAgent: true } }).text()).toContain(
      'nobody attached',
    )
  })

  it('reviews: marks stale approvals as no longer counting', () => {
    const p = proposal({ head: 'new' })
    const reviews = [
      {
        id: 1,
        proposal: p.id,
        reviewer: 'user:bob',
        decision: 'approve',
        body: 'looks good',
        head: 'old',
        timestamp_ms: 1,
      },
      {
        id: 2,
        proposal: p.id,
        reviewer: 'user:carol',
        decision: 'approve',
        body: '',
        head: 'new',
        timestamp_ms: 2,
      },
    ]
    const w = mount(ReviewsTab, {
      props: {
        proposal: p,
        reviews,
        summary: {
          current: [reviews[1]!],
          approvals: [reviews[1]!],
          changesRequested: [],
          staleApprovals: [reviews[0]!],
        },
        canReview: true,
        reviewReason: '',
      },
    })
    const items = w.findAll('[data-review]')
    expect(items).toHaveLength(2)
    expect(w.find('[data-review="1"]').classes()).toContain('stale')
    expect(w.find('[data-review="1"]').text()).toContain('no longer counts')
    expect(w.find('[data-review="2"]').classes()).not.toContain('stale')
    expect(w.text()).toContain('Some approvals are stale')
  })
  it('reviews: the button is unavailable (with the reason) without the role', () => {
    const w = mount(ReviewsTab, {
      props: {
        proposal: proposal(),
        reviews: [],
        summary: null,
        canReview: false,
        reviewReason: 'Requires the reviewer role',
      },
    })
    const btn = w.find('button')
    expect(btn.attributes('aria-disabled')).toBe('true')
    expect(btn.attributes('title')).toBe('Requires the reviewer role')
  })

  it('validation: shows required checks, evidence and the exact environment digest', () => {
    const prep = {
      proposal: 'p',
      strategy: 'fast_forward',
      candidate_tree: 'tree'.padEnd(64, '0'),
      conflicts: [],
      base: '',
      changed_files: [],
      problem: '',
      algorithm: '',
      policy_version: 2,
      blockers: [],
      validations: [{ check: 'tests', state: 'passed', attestation_id: 'att_abc', job_id: '' }],
      jobs: [],
    }
    const att = {
      id: 'att_abc',
      tree_id: prep.candidate_tree,
      check: 'tests',
      result: 'pass',
      policy_version: 2,
      environment_digest: 'sha256:' + 'ab'.repeat(32),
      issuer: 'service:ci',
      issued_at_ms: Date.now(),
      public_key: 'k'.repeat(64),
      signature: 's',
    }
    const w = mount(ValidationTab, {
      props: {
        prep,
        attestations: [att],
        canRequest: true,
        requestReason: '',
        requesting: false,
        open: true,
      },
    })
    expect(w.text()).toContain('tests')
    expect(w.text()).toContain('passed')
    expect(w.find('[data-attestation] [title="service:ci"]').exists()).toBe(true)
    expect(w.find('[data-attestation]').text()).toContain('sha256:' + 'ab'.repeat(32))
    expect(w.find('[data-attestation]').text()).toContain('Policy version')
  })
})

describe('lease typing sanity', () => {
  it('keeps the generated Lease shape we rely on', () => {
    const l: Lease = {
      id: 'l',
      scope: 's',
      holder: 'h',
      exclusive: true,
      fencing_token: 1,
      acquired_at_ms: 0,
      expires_at_ms: 1,
      released_at_ms: 0,
      task_id: '',
    }
    expect(l.fencing_token).toBe(1)
  })
})
