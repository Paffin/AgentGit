import { defineStore } from 'pinia'
import { computed, reactive, ref, shallowRef } from 'vue'
import { api } from '@/api/instance'
import type {
  Agent,
  Lease,
  Overlap,
  PolicyVersion,
  Preparation,
  Proposal,
  ProposalDetail,
  QueueEntry,
  Repo,
  Run,
  ServerEvent,
  Stream,
  Task,
} from '@/api/types'
import { deriveFourStates } from '@/lib/fourstate'
import type { FourStates } from '@/lib/fourstate'
import { sectionsFor } from '@/lib/events'
import type { Section } from '@/lib/events'
import { layoutDag } from '@/lib/dag'
import { scopesMayOverlap } from '@/lib/glob'
import { parseIdentity } from '@/lib/identity'
import { coalesce, mapLimit } from '@/lib/pool'
import { isActive } from '@/lib/runs'
import { summarizePolicy } from '@/lib/policy'
import { useLive } from './live'
import type { LiveMessage } from './live'

type Status = 'idle' | 'loading' | 'ready' | 'error'
export interface Load {
  status: Status
  error: unknown
}
export type BoardPart =
  | 'repo'
  | 'streams'
  | 'proposals'
  | 'tasks'
  | 'runs'
  | 'leases'
  | 'queue'
  | 'overlaps'
  | 'policy'
  | 'agents'

const PARTS: BoardPart[] = [
  'repo',
  'streams',
  'proposals',
  'tasks',
  'runs',
  'leases',
  'queue',
  'overlaps',
  'policy',
  'agents',
]

export interface PrepState {
  data?: Preparation
  error?: unknown
}

export interface ProposalCard {
  proposal: Proposal
  detail?: ProposalDetail
  prep?: PrepState
  states: FourStates
}

export interface LeaseRow {
  lease: Lease
  /** Other active leases (of other principals) whose scope may overlap this one. */
  overlaps: Lease[]
}

const POLL_MS = 8000
const DETAIL_CONCURRENCY = 4
/** Page sizes the board asks for; a full page of open proposals is reported as "more", never as the total. */
const OPEN_PAGE_SIZE = 100
const RUNS_LIMIT = 100

/**
 * Everything the live board shows, kept fresh by the repository's event stream: each event marks the
 * affected sections dirty, and one coalesced refresh re-reads only those. Heartbeats leave no trace in the
 * event log, so runs and leases are also polled while the tab is visible.
 */
export const useBoard = defineStore('board', () => {
  const repoName = ref<string | null>(null)
  const repo = shallowRef<Repo | null>(null)
  const streams = shallowRef<Stream[]>([])
  const stream = ref('')
  const proposals = shallowRef<Proposal[]>([])
  /** The open-proposals page was full: there are more open proposals than the board shows. */
  const proposalsTruncated = ref(false)
  const recent = shallowRef<Proposal[]>([])
  const details = shallowRef<Record<string, ProposalDetail>>({})
  const preps = shallowRef<Record<string, PrepState>>({})
  const tasks = shallowRef<Task[]>([])
  const runs = shallowRef<Run[]>([])
  const leases = shallowRef<Lease[]>([])
  const queue = shallowRef<QueueEntry[]>([])
  const overlaps = shallowRef<Overlap[]>([])
  const policy = shallowRef<PolicyVersion | null>(null)
  const agents = shallowRef<Agent[]>([])
  const load = reactive<Record<BoardPart, Load>>(
    Object.fromEntries(PARTS.map((p) => [p, { status: 'idle', error: null }])) as Record<
      BoardPart,
      Load
    >,
  )
  const lastSyncAt = ref(0)

  let unsubscribe: (() => void) | null = null
  let pollTimer: ReturnType<typeof setInterval> | undefined
  let session = 0
  const dirty = new Set<Section>()

  const live = useLive()

  // ---- derived ------------------------------------------------------------------------------------

  const policySummary = computed(() => summarizePolicy(policy.value?.document ?? '{}'))
  const streamProposals = computed(() =>
    proposals.value.filter((p) => !stream.value || p.stream === stream.value),
  )
  const cards = computed<ProposalCard[]>(() =>
    streamProposals.value.map((proposal) => {
      const detail = details.value[proposal.id]
      const prep = preps.value[proposal.id]
      return {
        proposal,
        detail,
        prep,
        states: deriveFourStates({
          proposal,
          prep: prep?.data,
          reviews: detail?.reviews ?? null,
        }),
      }
    }),
  )
  const activeRuns = computed(() => runs.value.filter(isActive))
  const taskLayout = computed(() => layoutDag(tasks.value))
  const activeLeases = computed(() => leases.value.filter((l) => l.released_at_ms === 0))
  const leaseRows = computed<LeaseRow[]>(() => {
    const list = [...activeLeases.value].sort(
      (a, b) => a.scope.localeCompare(b.scope) || a.acquired_at_ms - b.acquired_at_ms,
    )
    return list.map((lease) => ({
      lease,
      overlaps: list.filter(
        (o) =>
          o.id !== lease.id &&
          parseIdentity(o.holder).principal !== parseIdentity(lease.holder).principal &&
          scopesMayOverlap(o.scope, lease.scope),
      ),
    }))
  })
  const queueEntries = computed(() =>
    queue.value.filter((q) => !stream.value || q.stream === stream.value),
  )
  const taskById = computed(() => new Map(tasks.value.map((t) => [t.id, t])))
  const leasesByHolder = computed(() => {
    const m = new Map<string, Lease[]>()
    for (const l of activeLeases.value) {
      const key = parseIdentity(l.holder).raw
      const arr = m.get(key) ?? []
      arr.push(l)
      m.set(key, arr)
    }
    return m
  })

  // ---- loading ------------------------------------------------------------------------------------

  function mark(part: BoardPart, status: Status, error: unknown = null) {
    load[part].status = status
    load[part].error = error
  }

  async function guarded<T>(
    part: BoardPart,
    mine: number,
    fn: () => Promise<T>,
  ): Promise<T | undefined> {
    // Show the skeleton only for the first load; later refreshes keep the data on screen.
    if (load[part].status !== 'ready') mark(part, 'loading')
    try {
      const value = await fn()
      if (mine !== session) return undefined
      mark(part, 'ready')
      return value
    } catch (e) {
      if (mine !== session) return undefined
      mark(part, 'error', e)
      return undefined
    }
  }

  async function loadRepo(mine: number) {
    const r = repoName.value
    if (!r) return
    const value = await guarded('repo', mine, () => api.repos.get(r))
    if (value) {
      repo.value = value
      if (!stream.value) stream.value = value.default_stream
    }
  }
  async function loadStreams(mine: number) {
    const r = repoName.value
    if (!r) return
    const value = await guarded('streams', mine, () => api.streams.list(r))
    if (value) streams.value = value
  }
  async function loadPolicy(mine: number) {
    const r = repoName.value
    if (!r || !stream.value) return
    const value = await guarded('policy', mine, () => api.admin.policy(r, stream.value))
    if (value) policy.value = value
  }
  async function loadTasks(mine: number) {
    const r = repoName.value
    if (!r) return
    const value = await guarded('tasks', mine, () => api.agents.tasks(r))
    if (value) tasks.value = value
  }
  async function loadRuns(mine: number) {
    const r = repoName.value
    if (!r) return
    // The server applies the filter before its limit, so a long-running agent is listed however many newer runs ended.
    const value = await guarded('runs', mine, () =>
      api.agents.runs(r, { active_only: true, limit: RUNS_LIMIT }),
    )
    if (value) runs.value = value
  }
  async function loadLeases(mine: number) {
    const r = repoName.value
    if (!r) return
    const value = await guarded('leases', mine, () => api.agents.leases(r, true))
    if (value) leases.value = value
  }
  async function loadQueue(mine: number) {
    const r = repoName.value
    if (!r || !stream.value) return
    const value = await guarded('queue', mine, () => api.integration.queue(r, stream.value))
    if (value) queue.value = value
  }
  async function loadOverlaps(mine: number) {
    const r = repoName.value
    if (!r || !stream.value) return
    const value = await guarded('overlaps', mine, () => api.agents.predict(r, stream.value))
    if (value) overlaps.value = value
  }
  async function loadAgents(mine: number) {
    const r = repoName.value
    if (!r) return
    const value = await guarded('agents', mine, () => api.agents.list(r))
    if (value) agents.value = value
  }

  async function loadDetails(mine: number, only?: string[]) {
    const r = repoName.value
    if (!r) return
    const ids = only ?? proposals.value.map((p) => p.id)
    const results = await mapLimit(ids, DETAIL_CONCURRENCY, (id) => api.proposals.get(r, id))
    if (mine !== session) return
    const next = { ...details.value }
    results.forEach((res, i) => {
      const id = ids[i]
      if (id && res.status === 'fulfilled') next[id] = res.value
    })
    for (const id of Object.keys(next))
      if (!proposals.value.some((p) => p.id === id)) delete next[id]
    details.value = next
  }

  async function loadPreps(mine: number, only?: string[]) {
    const r = repoName.value
    if (!r) return
    const ids = only ?? proposals.value.map((p) => p.id)
    const results = await mapLimit(ids, DETAIL_CONCURRENCY, (id) => api.integration.prepare(r, id))
    if (mine !== session) return
    const next = { ...preps.value }
    results.forEach((res, i) => {
      const id = ids[i]
      if (!id) return
      if (res.status === 'fulfilled') next[id] = { data: res.value }
      else next[id] = { data: next[id]?.data, error: res.reason }
    })
    for (const id of Object.keys(next))
      if (!proposals.value.some((p) => p.id === id)) delete next[id]
    preps.value = next
  }

  async function loadProposals(mine: number, withDerived = true) {
    const r = repoName.value
    if (!r) return
    const open = await guarded('proposals', mine, async () => {
      const [openPage, donePage] = await Promise.all([
        api.proposals.list(r, { state: 'open', page_size: OPEN_PAGE_SIZE }),
        api.proposals.list(r, { state: 'integrated', page_size: 6 }),
      ])
      return { open: openPage.items, done: donePage.items, more: openPage.nextPageToken !== '' }
    })
    if (!open) return
    proposals.value = open.open
    proposalsTruncated.value = open.more
    recent.value = open.done
    if (withDerived) await Promise.all([loadDetails(mine), loadPreps(mine)])
  }

  /** First load and full resync: everything, in parallel. */
  async function refreshAll() {
    const mine = session
    await loadRepo(mine)
    await Promise.all([
      loadStreams(mine),
      loadProposals(mine),
      loadTasks(mine),
      loadRuns(mine),
      loadLeases(mine),
      loadQueue(mine),
      loadOverlaps(mine),
      loadPolicy(mine),
      loadAgents(mine),
    ])
    if (mine === session) lastSyncAt.value = Date.now()
  }

  async function flush() {
    const mine = session
    const sections = new Set(dirty)
    dirty.clear()
    if (sections.has('proposals')) sections.delete('prepare-all') // covered: it re-prepares everything
    const jobs: Promise<unknown>[] = []
    for (const s of sections) {
      switch (s) {
        case 'proposals':
          jobs.push(loadProposals(mine))
          break
        case 'prepare-all':
          jobs.push(loadPreps(mine))
          break
        case 'streams':
          jobs.push(loadStreams(mine))
          break
        case 'runs':
          jobs.push(loadRuns(mine))
          break
        case 'leases':
          jobs.push(loadLeases(mine))
          break
        case 'tasks':
          jobs.push(loadTasks(mine))
          break
        case 'queue':
          jobs.push(loadQueue(mine))
          break
        case 'predict':
          jobs.push(loadOverlaps(mine))
          break
        case 'policy':
          jobs.push(loadPolicy(mine))
          break
        case 'agents':
          jobs.push(loadAgents(mine))
          break
      }
    }
    await Promise.all(jobs)
    if (mine === session) lastSyncAt.value = Date.now()
  }

  const flusher = coalesce(flush, 180)

  function markDirty(sections: Section[]) {
    for (const s of sections) dirty.add(s)
    if (sections.length > 0) flusher.trigger()
  }

  function onLive(m: LiveMessage) {
    if (m.kind === 'resync') {
      void refreshAll()
      return
    }
    handleEvent(m.event)
  }

  function handleEvent(ev: ServerEvent) {
    markDirty(sectionsFor(ev.topic))
  }

  function poll() {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
    const mine = session
    void loadRuns(mine)
    void loadLeases(mine)
  }

  function onVisible() {
    if (document.visibilityState === 'visible' && repoName.value) {
      poll()
      markDirty(['prepare-all'])
    }
  }

  async function start(name: string) {
    if (repoName.value === name) return
    stop()
    repoName.value = name
    session++
    for (const p of PARTS) mark(p, 'idle')
    unsubscribe = live.subscribe(onLive)
    pollTimer = setInterval(poll, POLL_MS)
    document.addEventListener('visibilitychange', onVisible)
    await live.start(name)
    await refreshAll()
  }

  function stop() {
    session++
    unsubscribe?.()
    unsubscribe = null
    clearInterval(pollTimer)
    pollTimer = undefined
    flusher.cancel()
    dirty.clear()
    document.removeEventListener('visibilitychange', onVisible)
    live.stop()
    repoName.value = null
    repo.value = null
    streams.value = []
    stream.value = ''
    proposals.value = []
    proposalsTruncated.value = false
    recent.value = []
    details.value = {}
    preps.value = {}
    tasks.value = []
    runs.value = []
    leases.value = []
    queue.value = []
    overlaps.value = []
    policy.value = null
    agents.value = []
    lastSyncAt.value = 0
  }

  async function selectStream(name: string) {
    stream.value = name
    const mine = session
    await Promise.all([loadQueue(mine), loadOverlaps(mine), loadPolicy(mine)])
  }

  /** After the user acted: re-read what the action touched without waiting for the event round trip. */
  function touched(sections: Section[]) {
    markDirty(sections)
  }

  return {
    repoName,
    repo,
    streams,
    stream,
    proposals,
    recent,
    details,
    preps,
    tasks,
    runs,
    leases,
    queue,
    overlaps,
    policy,
    agents,
    load,
    lastSyncAt,
    policySummary,
    proposalsTruncated,
    streamProposals,
    cards,
    activeRuns,
    taskLayout,
    activeLeases,
    leaseRows,
    queueEntries,
    taskById,
    leasesByHolder,
    start,
    stop,
    selectStream,
    refreshAll,
    touched,
    handleEvent,
  }
})
