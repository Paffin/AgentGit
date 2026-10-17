import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { ApiError } from '@/api/errors'
import type { FeedOptions } from '@/api/sse'
import type {
  Lease,
  Preparation,
  Proposal,
  ProposalDetail,
  Run,
  ServerEvent,
  Task,
} from '@/api/types'

// ---- doubles ----------------------------------------------------------------------------------------
const feed = { close: vi.fn(), lastSeq: null as number | null }
let feedOptions: FeedOptions | null = null
vi.mock('@/api/sse', () => ({
  openEventFeed: (o: FeedOptions) => {
    feedOptions = o
    return feed
  },
}))

const A = {
  replay: vi.fn(),
  repoGet: vi.fn(),
  streams: vi.fn(),
  proposalsList: vi.fn(),
  proposalGet: vi.fn(),
  prepare: vi.fn(),
  tasks: vi.fn(),
  runs: vi.fn(),
  leases: vi.fn(),
  queue: vi.fn(),
  predict: vi.fn(),
  policy: vi.fn(),
  agents: vi.fn(),
}
vi.mock('@/api/instance', () => ({
  client: {},
  api: {
    events: { replay: (...a: unknown[]) => A.replay(...a) },
    repos: { get: (...a: unknown[]) => A.repoGet(...a) },
    streams: { list: (...a: unknown[]) => A.streams(...a) },
    proposals: {
      list: (...a: unknown[]) => A.proposalsList(...a),
      get: (...a: unknown[]) => A.proposalGet(...a),
    },
    integration: {
      prepare: (...a: unknown[]) => A.prepare(...a),
      queue: (...a: unknown[]) => A.queue(...a),
    },
    agents: {
      tasks: (...a: unknown[]) => A.tasks(...a),
      runs: (...a: unknown[]) => A.runs(...a),
      leases: (...a: unknown[]) => A.leases(...a),
      predict: (...a: unknown[]) => A.predict(...a),
      list: (...a: unknown[]) => A.agents(...a),
    },
    admin: { policy: (...a: unknown[]) => A.policy(...a) },
  },
}))

import { useBoard } from './board'
import { useLive } from './live'

const flush = async (ms = 0) => {
  await vi.advanceTimersByTimeAsync(ms)
}

const ev = (seq: number, topic: string, payload: Record<string, unknown> = {}): ServerEvent => ({
  seq,
  topic,
  repo: 'acme/web',
  timestamp_ms: 1000 * seq,
  payload,
})

const proposal = (id: string, over: Partial<Proposal> = {}): Proposal => ({
  id,
  stream: 'main',
  head: `h-${id}`,
  title: `Proposal ${id}`,
  description: '',
  author: 'agent:bot#run_1',
  state: 'open',
  task_id: '',
  created_at_ms: 1,
  updated_at_ms: 1,
  integrated_as: '',
  ...over,
})
const detail = (p: Proposal): ProposalDetail => ({ proposal: p, reviews: [] })
const prep = (id: string, over: Partial<Preparation> = {}): Preparation => ({
  proposal: id,
  strategy: 'fast_forward',
  candidate_tree: 't',
  conflicts: [],
  base: '',
  changed_files: ['a'],
  problem: '',
  algorithm: '',
  policy_version: 1,
  blockers: [],
  validations: [],
  jobs: [],
  ...over,
})

function seed() {
  const p1 = proposal('p1')
  A.replay.mockImplementation(async (_repo: string, q: { after_seq?: number; limit?: number }) => ({
    events:
      q.after_seq === 0 && q.limit === 1
        ? []
        : [ev(41, 'proposal.create', { actor: 'agent:bot#run_1', target: 'p1' })],
    latestSeq: 42,
  }))
  A.repoGet.mockResolvedValue({ name: 'acme/web', default_stream: 'main', description: 'd' })
  A.streams.mockResolvedValue([
    {
      name: 'main',
      head: 'h0',
      version: 3,
      policy_version: 1,
      created_at_ms: 1,
      created_by: 'user:a',
    },
  ])
  A.proposalsList.mockImplementation(async (_r: string, q: { state?: string }) => ({
    items: q.state === 'open' ? [p1] : [],
    nextPageToken: '',
  }))
  A.proposalGet.mockImplementation(async (_r: string, id: string) => detail(proposal(id)))
  A.prepare.mockImplementation(async (_r: string, id: string) => prep(id))
  A.tasks.mockResolvedValue([] as Task[])
  A.runs.mockResolvedValue([] as Run[])
  A.leases.mockResolvedValue([] as Lease[])
  A.queue.mockResolvedValue([])
  A.predict.mockResolvedValue([])
  A.policy.mockResolvedValue({
    stream: 'main',
    version: 1,
    document: '{"approvals":{"min":1}}',
    doc_hash: '',
    created_at_ms: 1,
    created_by: 'x',
  })
  A.agents.mockResolvedValue([])
}

beforeEach(() => {
  vi.useFakeTimers()
  setActivePinia(createPinia())
  Object.values(A).forEach((m) => m.mockReset())
  feed.close.mockReset()
  feedOptions = null
  seed()
})

describe('live event stream', () => {
  it('baselines first, then follows from the newest event it saw (no gap, no replay of history)', async () => {
    const live = useLive()
    await live.start('acme/web')
    expect(A.replay).toHaveBeenCalledWith('acme/web', { after_seq: 0, limit: 1 })
    expect(feedOptions?.afterSeq).toBe(42)
    expect(live.latestSeq).toBe(42)
    expect(live.events).toHaveLength(1)
  })

  it('puts streamed events first, marked live, and tells subscribers', async () => {
    const live = useLive()
    await live.start('acme/web')
    const seen: string[] = []
    live.subscribe((m) => m.kind === 'event' && seen.push(m.event.topic))
    feedOptions!.onEvent(
      ev(43, 'lease.acquire', {
        actor: 'agent:bot',
        target: 'lease_1',
        details: { scope: 'src/**' },
      }),
    )
    expect(seen).toEqual(['lease.acquire'])
    expect(live.events[0]).toMatchObject({ topic: 'lease.acquire' })
    expect(live.events[0]!.liveAt).toBeGreaterThan(0)
    expect(live.latestSeq).toBe(43)
  })

  it('asks subscribers to resync after coming back from a reconnect', async () => {
    const live = useLive()
    await live.start('acme/web')
    const kinds: string[] = []
    live.subscribe((m) => kinds.push(m.kind))
    feedOptions!.onStatus?.('live', { attempt: 0 })
    feedOptions!.onStatus?.('reconnecting', { attempt: 1, retryInMs: 2000 })
    expect(live.status).toBe('reconnecting')
    expect(live.retryInMs).toBe(2000)
    feedOptions!.onStatus?.('live', { attempt: 1 })
    expect(kinds).toEqual(['resync'])
  })

  it('shows "no access" instead of retrying when the repository is hidden', async () => {
    A.replay.mockRejectedValue(new ApiError({ status: 404, code: 'not_found', message: 'nope' }))
    const live = useLive()
    await live.start('acme/web')
    expect(live.status).toBe('denied')
    expect(feedOptions).toBeNull()
  })

  it('a stopped feed cannot touch the next one', async () => {
    const live = useLive()
    await live.start('acme/web')
    const old = feedOptions!
    live.stop()
    expect(feed.close).toHaveBeenCalled()
    await live.start('acme/api')
    old.onStatus?.('closed', { attempt: 0 })
    old.onEvent(ev(99, 'x'))
    expect(live.status).not.toBe('closed')
    expect(live.latestSeq).toBe(42)
  })
})

describe('the board', () => {
  it('loads every section and derives a card per open proposal', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    expect(board.stream).toBe('main')
    expect(board.cards).toHaveLength(1)
    expect(board.cards[0]!.states.mergeable.tone).toBe('ok')
    expect(board.load.proposals.status).toBe('ready')
    expect(board.policySummary.approvalsMin).toBe(1)
    board.stop()
  })

  it('re-reads only what an event can have changed (a lease touches leases, not tasks)', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    A.tasks.mockClear()
    A.leases.mockClear()
    A.proposalsList.mockClear()
    feedOptions!.onEvent(ev(43, 'lease.acquire', { target: 'lease_9' }))
    await flush(400)
    expect(A.leases).toHaveBeenCalledTimes(1)
    expect(A.tasks).not.toHaveBeenCalled()
    expect(A.proposalsList).not.toHaveBeenCalled()
    board.stop()
  })

  it('coalesces a burst of events into one refresh', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    A.runs.mockClear()
    for (let i = 0; i < 10; i++)
      feedOptions!.onEvent(ev(50 + i, 'run.start', { target: `run_${i}` }))
    await flush(400)
    expect(A.runs).toHaveBeenCalledTimes(1)
    board.stop()
  })

  it('a new proposal appears without a reload, with its prepare and reviews fetched', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    const p2 = proposal('p2')
    A.proposalsList.mockImplementation(async (_r: string, q: { state?: string }) => ({
      items: q.state === 'open' ? [proposal('p1'), p2] : [],
      nextPageToken: '',
    }))
    feedOptions!.onEvent(ev(60, 'proposal.create', { target: 'p2' }))
    await flush(400)
    expect(board.cards.map((c) => c.proposal.id)).toEqual(['p1', 'p2'])
    expect(A.prepare).toHaveBeenCalledWith('acme/web', 'p2')
    board.stop()
  })

  it('flips the four states when the approval and validation arrive', async () => {
    A.prepare.mockImplementation(async (_r: string, id: string) =>
      prep(id, {
        validations: [{ check: 'tests', state: 'missing', attestation_id: '', job_id: '' }],
        blockers: [
          { code: 'missing_validation', message: 'm', detail: { check: 'tests' } },
          { code: 'insufficient_approvals', message: 'm', detail: { have: 0, required: 1 } },
        ],
      }),
    )
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    expect(board.cards[0]!.states.validated.tone).toBe('blocked')
    expect(board.cards[0]!.states.approved.tone).toBe('blocked')

    A.prepare.mockImplementation(async (_r: string, id: string) =>
      prep(id, {
        validations: [{ check: 'tests', state: 'passed', attestation_id: 'att', job_id: '' }],
      }),
    )
    A.proposalGet.mockImplementation(async (_r: string, id: string) => ({
      proposal: proposal(id),
      reviews: [
        {
          id: 1,
          proposal: id,
          reviewer: 'user:bob',
          decision: 'approve',
          body: '',
          head: `h-${id}`,
          timestamp_ms: 1,
        },
      ],
    }))
    feedOptions!.onEvent(ev(70, 'proposal.review', { target: 'p1' }))
    feedOptions!.onEvent(ev(71, 'attestation.submit', { target: 'tree' }))
    await flush(400)
    expect(board.cards[0]!.states.validated.tone).toBe('ok')
    expect(board.cards[0]!.states.approved.tone).toBe('ok')
    board.stop()
  })

  it('polls runs and leases because heartbeats leave no event', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    A.runs.mockClear()
    await flush(8100)
    expect(A.runs).toHaveBeenCalled()
    board.stop()
  })

  it('resyncs everything after a reconnect', async () => {
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    A.tasks.mockClear()
    feedOptions!.onStatus?.('live', { attempt: 0 })
    feedOptions!.onStatus?.('reconnecting', { attempt: 1 })
    feedOptions!.onStatus?.('live', { attempt: 1 })
    await flush(50)
    expect(A.tasks).toHaveBeenCalled()
    board.stop()
  })

  it('marks a section as failed without losing the rest, and keeps old data on a failed refresh', async () => {
    A.queue.mockRejectedValue(
      new ApiError({ status: 500, code: 'internal', message: 'queue down' }),
    )
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    expect(board.load.queue.status).toBe('error')
    expect(board.load.proposals.status).toBe('ready')
    expect(board.cards).toHaveLength(1)
    A.proposalsList.mockRejectedValue(
      new ApiError({ status: 500, code: 'internal', message: 'boom' }),
    )
    feedOptions!.onEvent(ev(80, 'proposal.update', { target: 'p1' }))
    await flush(400)
    expect(board.load.proposals.status).toBe('error')
    expect(board.cards).toHaveLength(1) // still showing what it knew
    board.stop()
  })

  it('computes lease overlaps between different principals only', async () => {
    const lease = (id: string, scope: string, holder: string, exclusive = false): Lease => ({
      id,
      scope,
      holder,
      exclusive,
      fencing_token: 1,
      acquired_at_ms: 1,
      expires_at_ms: 1e13,
      released_at_ms: 0,
      task_id: '',
    })
    A.leases.mockResolvedValue([
      lease('l1', 'src/**', 'agent:a#run_1', true),
      lease('l2', 'src/auth/**', 'agent:b#run_2'),
      lease('l3', 'docs/**', 'agent:b#run_2'),
      lease('l4', 'src/x/**', 'agent:a#run_9'),
    ])
    const board = useBoard()
    await board.start('acme/web')
    await flush(10)
    const overlaps = Object.fromEntries(
      board.leaseRows.map((r) => [r.lease.id, r.overlaps.map((o) => o.id).sort()]),
    )
    expect(overlaps.l1).toEqual(['l2'])
    expect(overlaps.l2).toEqual(['l1'])
    expect(overlaps.l3).toEqual([])
    expect(overlaps.l4).toEqual([]) // same principal as l1, and src/x/ cannot meet src/auth/
    board.stop()
  })
})
