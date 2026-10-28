<script setup lang="ts">
import type { Proposal } from '@/api/types'
import type { LineQuote } from '@/lib/diffDiscussion'
import { api } from '@/api/instance'
import { useAuth } from '@/stores/auth'
import { usePagedResource } from '@/composables/usePagedResource'
import { useLiveReload } from '@/composables/useLiveReload'
import ThreadComposer from './ThreadComposer.vue'
import ReviewThreadCard from './ReviewThreadCard.vue'
import Button from '@/ui/Button.vue'
import ErrorState from '@/ui/ErrorState.vue'
const props = defineProps<{ repo: string; proposal: Proposal; quote?: LineQuote | null }>()
const emit = defineEmits<{ quoted: []; general: [quote: LineQuote, body: string] }>()
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
      @created="threads.reload()"
    />
    <ErrorState
      v-if="threads.error.value"
      compact
      what="line discussions"
      :error="threads.error.value"
      @retry="threads.reload()"
    />
    <ReviewThreadCard
      v-for="thread in threads.data.value"
      :key="thread.id"
      :repo="repo"
      :proposal="proposal"
      :thread="thread"
      @changed="threads.reload()"
    />
    <p
      v-if="!threads.data.value.length && threads.settled.value && !threads.error.value"
      class="muted"
    >
      No line discussions yet. Select a line number in Files changed to start one.
    </p>
    <Button v-if="threads.next.value" :loading="threads.loading.value" @click="threads.more"
      >Load older discussions</Button
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
.muted {
  color: var(--text-2);
}
</style>
