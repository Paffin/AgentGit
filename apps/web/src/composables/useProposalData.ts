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

/** How many reflog entries are searched for the move that integrated a proposal (the server's page size cap). */
const REFLOG_WINDOW = 500

/**
 * What the proposal's diff starts from. Open or closed: the merge base with its stream. Integrated: the
 * stream head it replaced (from the reflog), so the diff stays the change as it landed.
 *
 * A failed lookup is an error, never an empty base: an empty base would show the whole tree as added. `''` (the
 * empty tree) is only answered when the server says there is no common ancestor / the landed change has no parent.
 */
export async function diffBase(repo: string, p: Proposal): Promise<string> {
  if (p.state !== 'integrated') {
    // A branch nothing has landed on has no common ancestor with anything: the base is the empty tree.
    if (!(await api.streams.head(repo, p.stream)).head) return ''
    const bases = await api.history.mergeBase(repo, p.stream, p.head)
    return bases[0] ?? ''
  }
  const landed = p.integrated_as || p.head
  const log: ReflogEntry[] = await api.streams.reflog(repo, p.stream, REFLOG_WINDOW)
  const hit = log.find((e) => e.new_head === landed)
  if (hit) return hit.old_head
  // Older than the reflog window: the landed changeset's first parent is the stream head it replaced when it was
  // a fast-forward; for a merge it is the best base the history gives.
  const cs = await api.history.changeset(repo, landed)
  return cs.parents[0] ?? ''
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

  const detail = useResource<ProposalDetail>(
    (signal) => api.proposals.get(repo.value, id.value, { signal }),
    {
      watch: [repo, id],
      enabled: () =>
        !!id.value && repo.value.split('/').length === 2 && repo.value.split('/').every(Boolean),
    },
  )
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

  const diff = useResource<DiffResponse & { base: string; head: string }>(
    async (signal) => {
      const p = proposal.value
      const repository = repo.value
      if (!p) return { files: [], truncated: false, base: '', head: '' }
      const base = await diffBase(repository, p)
      signal.throwIfAborted()
      const res = await api.tree.diff(
        repository,
        {
          from: base,
          to: p.integrated_as || p.head,
          context: 3,
        },
        { signal },
      )
      return { ...res, base, head: p.integrated_as || p.head }
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
  /** Set when the leases could not be read: the integrate dialog must not pretend the author holds none. */
  const leasesError = ref<string>('')
  async function loadAuthorLeases() {
    const p = proposal.value
    if (!p) return
    try {
      const all = await api.agents.leases(repo.value, true)
      const who = p.author.split('#')[0]
      authorLeases.value = all.filter((l) => l.exclusive && l.holder.split('#')[0] === who)
      leasesError.value = ''
    } catch (e) {
      authorLeases.value = []
      leasesError.value = e instanceof Error ? e.message : 'the leases could not be read'
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
    leasesError,
    reloadAll,
    refresh: () => refresher.trigger(),
  }
}
