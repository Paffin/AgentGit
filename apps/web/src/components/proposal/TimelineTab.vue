<script setup lang="ts">
import { computed } from 'vue'
import { api } from '@/api/instance'
import type { Proposal } from '@/api/types'
import { describeEvent, eventConcernsProposal } from '@/lib/events'
import type { EventView } from '@/lib/events'
import { useResource } from '@/composables/useResource'
import { useLive } from '@/stores/live'
import ActivityRow from '@/components/ActivityRow.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ repo: string; proposal: Proposal }>()
const live = useLive()

// The live buffer holds the newest events; older ones come from a replay (the server cannot filter by target).
const WINDOW = 2500
const history = useResource<{ views: EventView[]; olderNotSearched: boolean }>(async () => {
  const head = await api.events.replay(props.repo, { after_seq: 0, limit: 1 })
  const from = Math.max(0, head.latestSeq - WINDOW)
  const page = await api.events.replay(props.repo, { after_seq: from, limit: WINDOW })
  return { views: page.events.map(describeEvent), olderNotSearched: from > 0 }
})

const items = computed(() => {
  const seen = new Set<number>()
  const out: EventView[] = []
  for (const e of [...live.events, ...(history.data.value?.views ?? [])]) {
    if (seen.has(e.seq) || !eventConcernsProposal(e, props.proposal.id, props.proposal.head))
      continue
    seen.add(e.seq)
    out.push(e)
  }
  return out.sort((a, b) => b.seq - a.seq)
})
</script>

<template>
  <div data-testid="timeline-tab">
    <p v-if="history.data.value?.olderNotSearched" class="faint note" data-testid="timeline-window">
      Only the last {{ WINDOW }} events of the repository were searched; an older proposal may have
      earlier events that are not listed here.
    </p>
    <div v-if="!history.settled.value && items.length === 0" class="sk">
      <Skeleton :lines="4" />
    </div>
    <ErrorState
      v-else-if="history.error.value && items.length === 0"
      what="the timeline"
      :error="history.error.value"
      compact
      @retry="history.reload()"
    />
    <EmptyState
      v-else-if="items.length === 0"
      icon="history"
      compact
      title="No events for this proposal yet"
    />
    <ol v-else class="tl" aria-label="Events for this proposal, newest first">
      <li v-for="e in items" :key="e.seq" class="r">
        <ActivityRow :event="e" :repo="repo" />
      </li>
    </ol>
    <p class="note faint">
      Showing events from the last ~2,500 in the repository; the audit log has the complete record.
    </p>
  </div>
</template>

<style scoped>
.tl {
  list-style: none;
  margin: 0;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface);
  overflow: hidden;
}
.r {
  height: 52px;
}
.r:last-child :deep(.row) {
  border-bottom: none;
}
.sk {
  padding: var(--s-3);
}
.note {
  margin-top: var(--s-2);
  font-size: var(--fs-xs);
}
</style>
