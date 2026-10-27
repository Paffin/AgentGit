<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import type { QueueEntry } from '@/api/types'
import { requirementText } from '@/lib/roles'
import { shortId } from '@/lib/format'
import { QUEUE_TONE, summarizeQueueRun } from '@/lib/queue'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ repo: string }>()
const board = useBoard()
const auth = useAuth()
const toasts = useToasts()
const running = ref(false)

const titleOf = computed(() => {
  const m = new Map<string, string>()
  for (const p of [...board.recent, ...board.proposals]) m.set(p.id, p.title)
  return m
})

const waiting = computed(() => board.queueEntries.filter((e) => e.state === 'queued').length)
const canRun = computed(() => auth.can(props.repo, 'runQueue'))

async function run() {
  running.value = true
  try {
    const res = await api.integration.runQueue(props.repo, board.stream)
    const s = summarizeQueueRun(res.processed)
    if (s.tone === 'warn') toasts.warn(s.title, s.detail)
    else toasts.ok(s.title)
    board.touched(['queue', 'proposals', 'streams'])
  } catch (e) {
    toasts.error('Could not run the queue', e)
  } finally {
    running.value = false
  }
}

const label = (e: QueueEntry) => titleOf.value.get(e.proposal) ?? shortId(e.proposal, 10)
</script>

<template>
  <Card
    title="Landing queue"
    icon="stack"
    :subtitle="`${board.stream || 'branch'} · ${waiting} waiting`"
    flush
  >
    <template #actions>
      <Button
        size="sm"
        icon="play"
        :loading="running"
        :disabled="!canRun || waiting === 0"
        :disabled-reason="!canRun ? requirementText('runQueue') : 'Nothing is waiting'"
        @click="run"
      >
        Run queue
      </Button>
    </template>
    <div class="body" data-testid="queue-panel">
      <div
        v-if="board.load.queue.status === 'loading' && board.queueEntries.length === 0"
        class="sk"
      >
        <Skeleton :height="40" />
      </div>
      <ErrorState
        v-else-if="board.load.queue.status === 'error' && board.queueEntries.length === 0"
        what="the queue"
        :error="board.load.queue.error"
        compact
        @retry="board.refreshAll()"
      />
      <EmptyState
        v-else-if="board.queueEntries.length === 0"
        icon="stack"
        compact
        title="The queue is empty"
        description="Queued change requests land in order. A blocked or rejected one is marked and skipped; the rest continue."
      />
      <ol v-else class="list">
        <li v-for="(e, i) in board.queueEntries" :key="e.id" class="entry" :data-queue="e.id">
          <span class="n">{{ i + 1 }}</span>
          <div class="main">
            <RouterLink
              :to="`/repos/${repo}/proposals/${encodeURIComponent(e.proposal)}`"
              class="t"
              >{{ label(e) }}</RouterLink
            >
            <p class="m faint">
              <IdentityLabel :identity="e.enqueued_by" :size="16" /> queued
              <RelativeTime :ms="e.enqueued_at_ms" />
              <template v-if="e.outcome"> · {{ e.outcome }}</template>
            </p>
          </div>
          <Badge :tone="QUEUE_TONE[e.state] ?? 'neutral'" size="sm">{{ e.state }}</Badge>
        </li>
      </ol>
    </div>
  </Card>
</template>

<style scoped>
.body {
  min-height: 110px;
}
.sk {
  padding: var(--s-4);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.entry {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
}
.n {
  flex: none;
  width: 22px;
  height: 22px;
  display: grid;
  place-items: center;
  border-radius: 50%;
  background: var(--surface-3);
  color: var(--text-2);
  font-size: var(--fs-xs);
  font-weight: 650;
}
.main {
  flex: 1;
  min-width: 0;
}
.t {
  font-weight: 580;
  color: var(--text);
  overflow-wrap: anywhere;
}
.m {
  font-size: var(--fs-xs);
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}
</style>
