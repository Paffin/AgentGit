<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import GettingStarted from '@/components/GettingStarted.vue'
import ProposalCard from '@/components/ProposalCard.vue'
import ActivityFeed from '@/components/board/ActivityFeed.vue'
import BoardSummary from '@/components/board/BoardSummary.vue'
import LeaseMap from '@/components/board/LeaseMap.vue'
import OverlapPanel from '@/components/board/OverlapPanel.vue'
import QueuePanel from '@/components/board/QueuePanel.vue'
import RunsPanel from '@/components/board/RunsPanel.vue'
import TaskGraph from '@/components/board/TaskGraph.vue'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { shortId } from '@/lib/format'
import { useBoard } from '@/stores/board'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'

const board = useBoard()
const { repo } = useRepoName()
usePageTitle(() => `Overview · ${repo.value}`)

// A repository nobody has pushed to: the first thing to do is to connect an agent. Only shown when the server said
// the counts are real (never on a guess) and nothing is open or running.
const untouched = computed(
  () =>
    board.repo !== null &&
    !board.repo.counts_error &&
    board.repo.changeset_count === 0 &&
    board.load.proposals.status === 'ready' &&
    board.cards.length === 0 &&
    board.recent.length === 0,
)

// The checklist stays while the repository is new (or the person just created it) and is not finished.
const route = useRoute()
const showStart = computed(
  () =>
    board.repo !== null &&
    !board.repo.counts_error &&
    board.load.proposals.status === 'ready' &&
    (untouched.value || route.query.welcome === '1' || board.repo.changeset_count === 0) &&
    board.recent.length === 0,
)

const loadingProposals = computed(
  () => board.load.proposals.status !== 'ready' && board.cards.length === 0,
)

// The panels further down appear once the ones above have their data, so nothing already on screen is
// pushed around while the board fills in (no layout shift). A repository that is slow to answer does not
// hold them back for ever.
const waiting = (status: string) => status === 'idle' || status === 'loading'
const settled = computed(() => {
  const l = board.load
  if (waiting(l.proposals.status) || waiting(l.runs.status) || waiting(l.leases.status))
    return false
  return board.cards.every((c) => c.proposal.state !== 'open' || c.prep?.data || c.prep?.error)
})
const showRest = ref(false)
let giveUp: ReturnType<typeof setTimeout> | undefined
watch(
  [settled, repo],
  ([ok], [, before]) => {
    if (before !== undefined && before !== repo.value) showRest.value = false
    if (ok) showRest.value = true
    else if (!showRest.value) {
      clearTimeout(giveUp)
      giveUp = setTimeout(() => (showRest.value = true), 6000)
    }
  },
  { immediate: true },
)
onBeforeUnmount(() => clearTimeout(giveUp))
</script>

<template>
  <div class="board">
    <h1 class="sr-only">Overview of {{ repo }}</h1>
    <BoardSummary />
    <GettingStarted v-if="showStart" :repo="repo" />

    <div class="grid">
      <div class="col main">
        <Card
          title="Open change requests"
          icon="proposal"
          subtitle="Each is checked on four separate counts; one being fine does not make another fine"
        >
          <template #actions>
            <RouterLink :to="`/repos/${repo}/proposals`" class="all"
              >All change requests</RouterLink
            >
          </template>
          <div class="cards" data-testid="proposals-panel">
            <template v-if="loadingProposals">
              <Skeleton :height="132" radius="10px" />
              <Skeleton :height="132" radius="10px" />
            </template>
            <ErrorState
              v-else-if="board.load.proposals.status === 'error' && board.cards.length === 0"
              what="change requests"
              :error="board.load.proposals.error"
              compact
              @retry="board.refreshAll()"
            />
            <EmptyState
              v-else-if="board.cards.length === 0"
              icon="proposal"
              title="No open change requests"
              description="When an agent or a person proposes a change it appears here, with whether it merges cleanly, passed its checks and is approved. To start one yourself, edit a file under Code."
            />
            <TransitionGroup v-else name="card" tag="div" class="stack">
              <ProposalCard
                v-for="c in board.cards"
                :key="c.proposal.id"
                :repo="repo"
                :proposal="c.proposal"
                :detail="c.detail"
                :prep="c.prep"
                :states="c.states"
              />
            </TransitionGroup>
          </div>
          <template v-if="board.recent.length" #footer>
            <div class="recent">
              <span class="faint">Recently landed</span>
              <ul role="list">
                <li v-for="p in board.recent.slice(0, 4)" :key="p.id">
                  <RouterLink :to="`/repos/${repo}/proposals/${encodeURIComponent(p.id)}`">{{
                    p.title
                  }}</RouterLink>
                  <span class="faint"
                    ><IdentityLabel :identity="p.author" :size="14" hide-run />
                    <RelativeTime :ms="p.updated_at_ms"
                  /></span>
                  <code class="mono faint">{{ shortId(p.integrated_as, 7) }}</code>
                </li>
              </ul>
            </div>
          </template>
        </Card>
        <template v-if="showRest">
          <TaskGraph />
          <div class="pair">
            <QueuePanel :repo="repo" />
            <OverlapPanel :repo="repo" />
          </div>
        </template>
      </div>

      <div class="col side">
        <RunsPanel :repo="repo" />
        <template v-if="showRest">
          <LeaseMap />
          <ActivityFeed :repo="repo" />
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.board {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.grid {
  display: grid;
  grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
  gap: var(--s-4);
  align-items: start;
}
.col {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  min-width: 0;
}
.pair {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--s-4);
}
.cards {
  min-height: 150px;
}
.stack {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.all {
  font-size: var(--fs-sm);
  font-weight: 560;
}
.recent {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.recent ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.recent li {
  display: flex;
  gap: var(--s-3);
  align-items: baseline;
  justify-content: space-between;
  font-size: var(--fs-sm);
}
.recent li a {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-enter-active {
  transition:
    opacity var(--dur-2) var(--ease),
    transform var(--dur-2) var(--ease);
}
.card-enter-from {
  opacity: 0;
  transform: translateY(-6px);
}
.card-move {
  transition: transform var(--dur-2) var(--ease);
}
@media (max-width: 1180px) {
  .grid {
    grid-template-columns: minmax(0, 1fr);
  }
  .pair {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
