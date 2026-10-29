<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { Proposal } from '@/api/types'
import type { LineQuote } from '@/lib/diffDiscussion'
import { api } from '@/api/instance'
import { useAuth } from '@/stores/auth'
import { usePagedResource } from '@/composables/usePagedResource'
import { useLiveReload } from '@/composables/useLiveReload'
import ThreadComposer from './ThreadComposer.vue'
import ReviewThreadCard from './ReviewThreadCard.vue'
import BatchSuggestions from './BatchSuggestions.vue'
import Button from '@/ui/Button.vue'
import ErrorState from '@/ui/ErrorState.vue'
const props = defineProps<{ repo: string; proposal: Proposal; quote?: LineQuote | null }>()
const emit = defineEmits<{ quoted: []; applied: []; general: [quote: LineQuote, body: string] }>()
const auth = useAuth()
const threads = usePagedResource(
  (cursor, signal) =>
    api.proposals.threads(
      props.repo,
      props.proposal.id,
      { page_size: 20, page_token: cursor },
      { signal },
    ),
  [() => props.repo, () => props.proposal.id, () => auth.token],
)
useLiveReload(['proposal'], () => threads.reload())
const filter = ref<'all' | 'open' | 'resolved'>('all')
const filters = ['all', 'open', 'resolved'] as const
const counts = computed(() => {
  const all = threads.data.value.length
  const resolved = threads.data.value.filter((thread) => thread.resolved).length
  return { all, open: all - resolved, resolved }
})
function showCreatedThread() {
  filter.value = 'all'
  void threads.reload()
}
watch([() => props.repo, () => props.proposal.id, () => auth.token], () => {
  filter.value = 'all'
})
</script>

<template>
  <section aria-labelledby="line-discussions-heading" class="threads">
    <header>
      <h2 id="line-discussions-heading">Line discussions</h2>
      <Button size="sm" :loading="threads.loading.value" @click="threads.reload()"
        >Refresh discussions</Button
      >
    </header>
    <ThreadComposer
      :repo="repo"
      :proposal="proposal"
      :quote="quote"
      @quoted="emit('quoted')"
      @general="(quote, body) => emit('general', quote, body)"
      @created="showCreatedThread"
    />
    <ErrorState
      v-if="threads.error.value"
      compact
      what="line discussions"
      :error="threads.error.value"
      @retry="threads.reload()"
    />
    <div v-if="threads.data.value.length" class="thread-filters">
      <div role="group" aria-label="Filter line discussions" class="filter-buttons">
        <Button
          v-for="choice in filters"
          :key="choice"
          size="sm"
          :variant="filter === choice ? 'primary' : 'secondary'"
          :aria-pressed="filter === choice"
          :label="choice === 'all' ? 'All' : choice === 'open' ? 'Open' : 'Resolved'"
          @click="filter = choice"
        >
          {{ choice === 'all' ? 'All' : choice === 'open' ? 'Open' : 'Resolved' }}
          <span aria-hidden="true">{{ counts[choice] }}</span>
        </Button>
      </div>
      <span v-if="threads.next.value" class="muted">Counts cover loaded discussions.</span>
    </div>
    <p
      v-if="
        filter !== 'all' && counts[filter] === 0 && threads.settled.value && !threads.error.value
      "
      role="status"
      class="muted"
    >
      No {{ filter }} discussions in the loaded results.
    </p>
    <BatchSuggestions
      :repo="repo"
      :proposal="proposal"
      :threads="threads.data.value"
      @applied="emit('applied')"
    />
    <ReviewThreadCard
      v-for="thread in threads.data.value"
      v-show="filter === 'all' || thread.resolved === (filter === 'resolved')"
      :key="thread.id"
      :repo="repo"
      :proposal="proposal"
      :thread="thread"
      @changed="threads.reload()"
      @applied="emit('applied')"
    />
    <p
      v-if="!threads.data.value.length && threads.settled.value && !threads.error.value"
      class="muted"
    >
      No line discussions yet. Select a line number in Files changed to start one.
    </p>
    <Button v-if="threads.next.value" :loading="threads.loading.value" @click="threads.more"
      >Load more discussions</Button
    >
  </section>
</template>

<style scoped>
.threads {
  display: grid;
  gap: var(--s-4);
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.thread-filters,
.filter-buttons {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.muted {
  color: var(--text-2);
}
</style>
