import { onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { api } from '@/api/instance'
import type { Preparation, Repo, Review } from '@/api/types'
import { attentionForRepo } from '@/lib/attention'
import type { AttentionItem } from '@/lib/attention'
import { mapLimit } from '@/lib/pool'
import { useAuth } from '@/stores/auth'
import { serverNow } from '@/api/clock'

/** How much one home page reads: it is a summary, not an inventory. */
const MAX_REPOS = 30
const MAX_PROPOSALS_PER_REPO = 12
const POLL_MS = 15_000

/**
 * What needs the signed-in person, across the repositories they can read. There is one event stream per
 * repository and the home page spans many, so it re-reads every 15 seconds while visible instead of following
 * them all; the page says so.
 */
export function useAttention() {
  const auth = useAuth()
  const repos = shallowRef<Repo[]>([])
  const items = shallowRef<AttentionItem[]>([])
  const loading = ref(true)
  const settled = ref(false)
  const refreshing = ref(false)
  const lastUpdated = ref<number | null>(null)
  const error = shallowRef<unknown>(null)
  /** Repositories the summary did not read because there are more than it reads. */
  const skipped = ref(0)
  /** Repositories whose data could not be read this time (the rest is still shown). */
  const unreadable = ref(0)
  let serial = 0
  let timer: ReturnType<typeof setInterval> | undefined

  async function load() {
    const mine = ++serial
    refreshing.value = true
    try {
      if (!auth.isAuthenticated) return
      const page = await api.repos.list({ page_size: 100 })
      if (mine !== serial) return
      repos.value = page.items
      // The newest repositories first: they are the ones most likely to have something going on.
      const read = [...page.items]
        .sort((a, b) => b.created_at_ms - a.created_at_ms)
        .slice(0, MAX_REPOS)
      skipped.value = Math.max(0, page.items.length - read.length) + (page.nextPageToken ? 1 : 0)
      const now = serverNow()
      let failures = 0
      // A sign-out, or a newer load, ends this one: no request goes out without a session.
      const stale = () => mine !== serial || !auth.isAuthenticated
      const perRepo = await mapLimit(read, 3, async (r) => {
        if (stale()) return []
        const may = (a: Parameters<typeof auth.can>[1]) => auth.can(r.name, a)
        const [proposals, runs] = await Promise.all([
          api.proposals.list(r.name, { state: 'open', page_size: MAX_PROPOSALS_PER_REPO }),
          api.agents.runs(r.name, { limit: 30 }),
        ])
        const preps: Record<string, Preparation | undefined> = {}
        const reviews: Record<string, readonly Review[] | undefined> = {}
        await mapLimit(proposals.items, 4, async (p) => {
          if (stale()) return
          const [prep, detail] = await Promise.all([
            api.integration.prepare(r.name, p.id),
            api.proposals.get(r.name, p.id),
          ])
          preps[p.id] = prep
          reviews[p.id] = detail.reviews
        })
        return attentionForRepo({
          repo: r.name,
          proposals: proposals.items,
          preps,
          reviews,
          runs,
          me: auth.identity,
          may,
          now,
        })
      })
      if (mine !== serial) return
      const all: AttentionItem[] = []
      for (const res of perRepo) {
        if (res.status === 'fulfilled') all.push(...res.value)
        else failures++
      }
      items.value = all
      unreadable.value = failures
      error.value = null
      lastUpdated.value = serverNow()
    } catch (e) {
      if (mine !== serial) return
      error.value = e
    } finally {
      if (mine === serial) {
        loading.value = false
        settled.value = true
        refreshing.value = false
      }
    }
  }

  onMounted(() => {
    void load()
    timer = setInterval(() => {
      if (document.visibilityState === 'visible' && !refreshing.value) void load()
    }, POLL_MS)
  })
  onBeforeUnmount(() => {
    serial++
    clearInterval(timer)
  })

  return {
    repos,
    items,
    loading,
    settled,
    refreshing,
    lastUpdated,
    error,
    skipped,
    unreadable,
    reload: load,
  }
}
