import { onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { api } from '@/api/instance'
import type { Preparation, Proposal, Repo, Review } from '@/api/types'
import { attentionForRepo } from '@/lib/attention'
import type { AttentionItem } from '@/lib/attention'
import { mapLimit } from '@/lib/pool'
import { useAuth } from '@/stores/auth'
import { serverNow } from '@/api/clock'

const MAX_REPOS = 30
const MAX_PROPOSALS_PER_REPO = 12
const MAX_RUNS = 30
const POLL_MS = 15_000

type Access = Pick<ReturnType<typeof useAuth>, 'identity' | 'can'>
interface Summary {
  items: AttentionItem[]
  incomplete: boolean
}

async function proposalFacts(repo: string, proposal: Proposal) {
  const [prep, detail] = await Promise.all([
    api.integration.prepare(repo, proposal.id),
    api.proposals.get(repo, proposal.id),
  ])
  return { id: proposal.id, prep, reviews: detail.reviews }
}

function valueOr<T>(result: PromiseSettledResult<T>, fallback: T): T {
  return result.status === 'fulfilled' ? result.value : fallback
}

function hasFailures(results: readonly PromiseSettledResult<unknown>[]): boolean {
  return results.some((result) => result.status === 'rejected')
}

function detailMaps(
  details: PromiseSettledResult<Awaited<ReturnType<typeof proposalFacts>> | null>[],
) {
  const preps: Record<string, Preparation | undefined> = {}
  const reviews: Record<string, readonly Review[] | undefined> = {}
  for (const result of details) {
    const detail = valueOr(result, null)
    if (!detail) continue
    preps[detail.id] = detail.prep
    reviews[detail.id] = detail.reviews
  }
  return { preps, reviews }
}

function pollVisible(load: () => Promise<void>, busy: boolean) {
  if (document.visibilityState === 'visible' && !busy) void load()
}

/** A failed detail must not hide the other proposals, and an unreadable run list must not imply healthy agents. */
async function repositorySummary(
  repo: Repo,
  access: Access,
  stale: () => boolean,
  now: number,
): Promise<Summary> {
  if (stale()) return { items: [], incomplete: false }
  const [proposalResult, runResult] = await Promise.allSettled([
    api.proposals.list(repo.name, { state: 'open', page_size: MAX_PROPOSALS_PER_REPO }),
    api.agents.runs(repo.name, { limit: MAX_RUNS }),
  ])
  const page = valueOr(proposalResult, { items: [] as Proposal[], nextPageToken: '' })
  const proposals = page.items
  const runs = valueOr(runResult, [])
  const details = await mapLimit(proposals, 4, (p) =>
    stale() ? Promise.resolve(null) : proposalFacts(repo.name, p),
  )
  const { preps, reviews } = detailMaps(details)
  const incomplete =
    hasFailures([proposalResult, runResult, ...details]) ||
    Boolean(page.nextPageToken) ||
    runs.length === MAX_RUNS
  return {
    items: attentionForRepo({
      repo: repo.name,
      proposals,
      runs,
      preps,
      reviews,
      me: access.identity,
      may: (action) => access.can(repo.name, action),
      now,
    }),
    incomplete,
  }
}

function combine(results: PromiseSettledResult<Summary>[]): {
  items: AttentionItem[]
  unreadable: number
} {
  const items: AttentionItem[] = []
  let unreadable = 0
  for (const result of results) {
    if (result.status === 'rejected') {
      unreadable++
      continue
    }
    items.push(...result.value.items)
    if (result.value.incomplete) unreadable++
  }
  return { items, unreadable }
}

async function readSnapshot(access: Access, stale: () => boolean) {
  const page = await api.repos.list({ page_size: 100 })
  if (stale()) return null
  const read = [...page.items].sort((a, b) => b.created_at_ms - a.created_at_ms).slice(0, MAX_REPOS)
  const now = serverNow()
  const results = await mapLimit(read, 3, (r) => repositorySummary(r, access, stale, now))
  return {
    repos: page.items,
    ...combine(results),
    skipped: Math.max(0, page.items.length - read.length) + (page.nextPageToken ? 1 : 0),
  }
}

/** Bounded, visible-page polling; a complete snapshot is committed only by the newest request. */
export function useAttention() {
  const auth = useAuth()
  const repos = shallowRef<Repo[]>([])
  const items = shallowRef<AttentionItem[]>([])
  const loading = ref(true)
  const settled = ref(false)
  const lastUpdated = ref(0)
  const error = shallowRef<unknown>(null)
  const skipped = ref(0)
  const unreadable = ref(0)
  let serial = 0
  let timer: ReturnType<typeof setInterval> | undefined

  async function load() {
    const mine = ++serial
    const stale = () => mine !== serial || !auth.isAuthenticated
    loading.value = true
    try {
      if (stale()) return
      const snapshot = await readSnapshot(auth, stale)
      if (stale() || !snapshot) return
      repos.value = snapshot.repos
      items.value = snapshot.items
      unreadable.value = snapshot.unreadable
      skipped.value = snapshot.skipped
      error.value = null
      lastUpdated.value = serverNow()
    } catch (e) {
      if (!stale()) error.value = e
    } finally {
      if (mine === serial) {
        loading.value = false
        settled.value = true
      }
    }
  }

  onMounted(() => {
    void load()
    timer = setInterval(() => pollVisible(load, loading.value), POLL_MS)
  })
  onBeforeUnmount(() => {
    serial++
    clearInterval(timer)
  })
  return { repos, items, loading, settled, lastUpdated, error, skipped, unreadable, reload: load }
}
