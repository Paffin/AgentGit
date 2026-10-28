<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import type { Run } from '@/api/types'
import { useLiveReload } from '@/composables/useLiveReload'
import { useResource } from '@/composables/useResource'
import { useServerNow } from '@/composables/useNow'
import { formatDuration, shortId } from '@/lib/format'
import { runHealth } from '@/lib/runs'
import { useBoard } from '@/stores/board'
import { usePrefs } from '@/stores/prefs'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Segmented from '@/ui/Segmented.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const board = useBoard()
const prefs = usePrefs()
const now = useServerNow()

const scope = ref<'active' | 'all'>('active')
const limit = ref(100)
watch(
  () => props.repo,
  () => (limit.value = 100),
)

const runs = useResource<Run[]>(
  () => api.agents.runs(props.repo, { active_only: scope.value === 'active', limit: limit.value }),
  { watch: [() => props.repo, scope, limit] },
)
useLiveReload(['run', 'task'], () => runs.reload())
// Heartbeats are not events: keep ages honest.
const timer = setInterval(() => void runs.reload(), 10_000)
onBeforeUnmount(() => clearInterval(timer))

const columns: Column[] = [
  { key: 'identity', label: 'Run', width: 'minmax(0, 1.6fr)' },
  { key: 'task', label: 'Task', width: 'minmax(0, 1.6fr)', hideBelow: 800 },
  { key: 'status', label: 'Status', width: '8.5rem' },
  { key: 'started_at_ms', label: 'Started', width: '7rem', hideBelow: 900 },
  { key: 'heartbeat', label: 'Heartbeat', width: '9rem', hideBelow: 600 },
  { key: 'summary', label: 'Summary', width: 'minmax(0, 1.4fr)', hideBelow: 1100 },
]
const TONE: Record<string, 'ok' | 'info' | 'warn' | 'bad' | 'neutral'> = {
  running: 'ok',
  completed: 'info',
  failed: 'bad',
  stale: 'warn',
  abandoned: 'warn',
}
const rows = computed(() => runs.data.value ?? [])
const taskTitle = (r: Run) =>
  r.task_id ? (board.taskById.get(r.task_id)?.title ?? shortId(r.task_id, 12)) : ''
const health = (r: Run) => runHealth(r, now.value, prefs.staleAfterMs)
const options = [
  { value: 'active', label: 'Active' },
  { value: 'all', label: 'History' },
] as const
</script>

<template>
  <div class="runs">
    <div class="bar">
      <Segmented v-model="scope" legend="Show" :options="options" />
      <p class="faint">
        Quiet = running but silent for over {{ formatDuration(prefs.staleAfterMs) }} (set on the
        Live board).
      </p>
    </div>
    <ErrorState
      v-if="runs.error.value && !runs.data.value"
      what="agent sessions"
      :error="runs.error.value"
      @retry="runs.reload()"
    />
    <DataTable
      v-else
      :columns="columns"
      :rows="rows"
      :row-key="(r: Run) => r.run_id"
      caption="Agent runs"
      :loading="!runs.settled.value"
    >
      <template #cell-identity="{ row }">
        <IdentityLabel :identity="(row as Run).identity" />
        <p class="faint mono id">{{ (row as Run).run_id }}</p>
        <p v-if="(row as Run).status === 'running'" class="faint compact-heartbeat">
          Heartbeat <RelativeTime :ms="(row as Run).last_heartbeat_ms" />
        </p>
      </template>
      <template #cell-task="{ row }"
        ><RouterLink
          v-if="(row as Run).task_id"
          :to="`/repos/${repo.split('/').map(encodeURIComponent).join('/')}/tasks/${encodeURIComponent((row as Run).task_id)}`"
          >{{ taskTitle(row as Run) }}</RouterLink
        ><span v-else>–</span></template
      >
      <template #cell-status="{ row }">
        <Badge
          v-if="(row as Run).status === 'running' && health(row as Run).liveness === 'stale'"
          tone="warn"
          icon="alert"
          size="sm"
          >quiet</Badge
        >
        <Badge
          v-else
          :tone="TONE[(row as Run).status] ?? 'neutral'"
          size="sm"
          :dot="(row as Run).status === 'running'"
          >{{ (row as Run).status }}</Badge
        >
      </template>
      <template #cell-started_at_ms="{ row }"
        ><span class="faint"><RelativeTime :ms="(row as Run).started_at_ms" /></span
      ></template>
      <template #cell-heartbeat="{ row }">
        <span
          v-if="(row as Run).status === 'running'"
          :class="{ warn: health(row as Run).liveness === 'stale' }"
          ><RelativeTime :ms="(row as Run).last_heartbeat_ms"
        /></span>
        <span v-else class="faint">{{
          (row as Run).finished_at_ms
            ? `ended ${formatDuration((row as Run).finished_at_ms - (row as Run).started_at_ms)} after start`
            : '–'
        }}</span>
      </template>
      <template #cell-summary="{ row }"
        ><span class="faint">{{ (row as Run).summary }}</span></template
      >
      <template #empty>
        <EmptyState
          icon="bot"
          compact
          :title="scope === 'active' ? 'No active agent sessions' : 'No agent sessions yet'"
          description="An agent appears here when it starts working on a task. To get one going, create a task and connect an agent under Tasks."
        />
      </template>
    </DataTable>
    <div v-if="scope === 'all' && rows.length >= limit" class="more">
      <Button size="sm" @click="limit += 100">Show 100 more</Button>
    </div>
  </div>
</template>

<style scoped>
.runs {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.bar {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
  font-size: var(--fs-sm);
}
.id {
  font-size: var(--fs-xs);
}
.compact-heartbeat {
  display: none;
}
@media (max-width: 599px) {
  .compact-heartbeat {
    display: block;
  }
}
.warn {
  color: var(--warn);
  font-weight: 600;
}
.more {
  display: flex;
  justify-content: center;
}
</style>
