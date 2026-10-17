<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
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
usePageTitle(() => `Live board · ${repo.value}`)

const loadingProposals = computed(
  () => board.load.proposals.status !== 'ready' && board.cards.length === 0,
)
</script>

<template>
  <div class="board">
    <h1 class="sr-only">Live board for {{ repo }}</h1>
    <BoardSummary />

    <div class="grid">
      <div class="col main">
        <Card
          title="Open proposals"
          icon="proposal"
          :subtitle="
            board.cards.length ? 'Four independent states each; nothing implies another' : undefined
          "
        >
          <template #actions>
            <RouterLink :to="`/repos/${repo}/proposals`" class="all">All proposals</RouterLink>
          </template>
          <div class="cards" data-testid="proposals-panel">
            <template v-if="loadingProposals">
              <Skeleton :height="132" radius="10px" />
              <Skeleton :height="132" radius="10px" />
            </template>
            <ErrorState
              v-else-if="board.load.proposals.status === 'error' && board.cards.length === 0"
              what="proposals"
              :error="board.load.proposals.error"
              compact
              @retry="board.refreshAll()"
            />
            <EmptyState
              v-else-if="board.cards.length === 0"
              icon="proposal"
              title="No open proposals"
              description="When an agent or a person proposes a change it appears here with what is mergeable, validated and approved."
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
              <span class="faint">Recently integrated</span>
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
        <TaskGraph />
        <div class="pair">
          <QueuePanel :repo="repo" />
          <OverlapPanel :repo="repo" />
        </div>
      </div>

      <div class="col side">
        <RunsPanel :repo="repo" />
        <LeaseMap />
        <ActivityFeed :repo="repo" />
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
