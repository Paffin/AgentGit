import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { api } from '@/api/instance'
import type {
  Attestation,
  DiffResponse,
  Lease,
  Preparation,
  Proposal,
  ProposalDetail,
  ProvenanceEntry,
  ReflogEntry,
} from '@/api/types'
import { deriveFourStates, summarizeReviews } from '@/lib/fourstate'
import { coalesce } from '@/lib/pool'
import { useLive } from '@/stores/live'
import { useResource } from './useResource'

/**
 * What the proposal's diff starts from. Open or closed: the merge base with its stream. Integrated: the
 * stream head it replaced (from the reflog), so the diff stays the change as it landed.
 */
async function diffBase(repo: string, p: Proposal): Promise<string> {
  const none = (): string => ''
  if (p.state !== 'integrated') {
    const bases = await api.history.mergeBase(repo, p.stream, p.head).catch(() => [] as string[])
    return bases[0] ?? ''
  }
  const landed = p.integrated_as || p.head
  const log: ReflogEntry[] = await api.streams.reflog(repo, p.stream, 500).catch(() => [])
  const hit = log.find((e) => e.new_head === landed)
  if (hit) return hit.old_head
  const cs = await api.history.changeset(repo, landed).catch(() => null)
  return cs?.parents[0] ?? none()
}

const RELEVANT = [
  'proposal',
  'stream',
  'integrate',
  'attestation',
  'provenance',
  'policy',
  'lease',
  'queue',
  'job',
]

/** Everything the proposal page shows, kept fresh by the repository's event stream. */
export function useProposalData(repo: Ref<string>, id: Ref<string>) {
  const live = useLive()

  const detail = useResource<ProposalDetail>(() => api.proposals.get(repo.value, id.value), {
    watch: [repo, id],
  })
  const proposal = computed(() => detail.data.value?.proposal)
  const reviews = computed(() => detail.data.value?.reviews ?? [])

  const prep = useResource<Preparation | null>(
    async () => {
      const p = proposal.value
      if (!p || p.state !== 'open') return null
      return api.integration.prepare(repo.value, p.id)
    },
    { immediate: false },
  )

  const provenance = useResource<ProvenanceEntry[]>(
    async () => {
      const p = proposal.value
      return p ? api.agents.provenance(repo.value, p.head) : []
    },
    { immediate: false },
  )

  const attestations = useResource<Attestation[]>(
    async () => {
      const tree = prep.data.value?.candidate_tree
      return tree ? api.integration.attestations(repo.value, tree) : []
    },
    { immediate: false },
  )

  const diff = useResource<DiffResponse & { base: string }>(
    async () => {
      const p = proposal.value
      if (!p) return { files: [], truncated: false, base: '' }
      const base = await diffBase(repo.value, p)
      const res = await api.tree.diff(repo.value, {
        from: base,
        to: p.integrated_as || p.head,
        context: 3,
      })
      return { ...res, base }
    },
    { immediate: false },
  )

  const states = computed(() =>
    proposal.value
      ? deriveFourStates({
          proposal: proposal.value,
          prep: prep.data.value,
          reviews: reviews.value,
        })
      : null,
  )
  const reviewSummary = computed(() =>
    proposal.value ? summarizeReviews(reviews.value, proposal.value.head) : null,
  )

  /** Leases of the author's principal that are exclusive and active: their tokens may be needed to integrate. */
  const authorLeases = ref<Lease[]>([])
  async function loadAuthorLeases() {
    const p = proposal.value
    if (!p) return
    try {
      const all = await api.agents.leases(repo.value, true)
      const who = p.author.split('#')[0]
      authorLeases.value = all.filter((l) => l.exclusive && l.holder.split('#')[0] === who)
    } catch {
      authorLeases.value = []
    }
  }

  async function reloadAll() {
    await detail.reload()
    await Promise.all([prep.reload(), provenance.reload(), diff.reload(), loadAuthorLeases()])
    await attestations.reload()
  }

  // Once the proposal itself is known, load the parts that depend on it (and again for another proposal).
  let started = false
  watch([repo, id], () => {
    started = false
  })
  watch(proposal, async (p) => {
    if (!p || started) return
    started = true
    await Promise.all([prep.reload(), provenance.reload(), diff.reload(), loadAuthorLeases()])
    await attestations.reload()
  })

  const refresher = coalesce(async () => {
    const before = proposal.value?.head
    await detail.reload()
    if (proposal.value?.head !== before) await diff.reload()
    await Promise.all([prep.reload(), provenance.reload(), loadAuthorLeases()])
    await attestations.reload()
  }, 250)

  const unsubscribe = live.subscribe((m) => {
    if (m.kind === 'resync') {
      refresher.trigger()
      return
    }
    if (RELEVANT.includes(m.event.topic.split('.')[0] ?? '')) refresher.trigger()
  })
  onBeforeUnmount(() => {
    unsubscribe()
    refresher.cancel()
  })

  return {
    detail,
    proposal,
    reviews,
    prep,
    provenance,
    attestations,
    diff,
    states,
    reviewSummary,
    authorLeases,
    reloadAll,
    refresh: () => refresher.trigger(),
  }
}
