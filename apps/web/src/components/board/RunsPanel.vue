<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { useServerNow } from '@/composables/useNow'
import { formatDuration } from '@/lib/format'
import { runHealth } from '@/lib/runs'
import { useBoard } from '@/stores/board'
import { usePrefs } from '@/stores/prefs'
import Badge from '@/ui/Badge.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import Icon from '@/ui/Icon.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'
import Skeleton from '@/ui/Skeleton.vue'

defineProps<{ repo: string }>()
const board = useBoard()
const prefs = usePrefs()
const now = useServerNow()

const rows = computed(() =>
  board.activeRuns
    .map((run) => ({
      run,
      health: runHealth(run, now.value, prefs.staleAfterMs),
      task: run.task_id ? board.taskById.get(run.task_id) : undefined,
      leases: board.leasesByHolder.get(run.identity) ?? [],
    }))
    .sort(
      (a, b) =>
        Number(b.health.liveness === 'stale') - Number(a.health.liveness === 'stale') ||
        b.run.started_at_ms - a.run.started_at_ms,
    ),
)
// Rows show their leases, so wait for both before showing any (otherwise each row grows when its chips arrive).
const waiting = (status: string) => status === 'idle' || status === 'loading'
const pending = computed(() => waiting(board.load.runs.status) || waiting(board.load.leases.status))
const staleCount = computed(() => rows.value.filter((r) => r.health.liveness === 'stale').length)

const STALE_OPTIONS = [
  { value: '30', label: '30 seconds' },
  { value: '60', label: '1 minute' },
  { value: '120', label: '2 minutes' },
  { value: '300', label: '5 minutes' },
  { value: '600', label: '10 minutes' },
]
const staleSeconds = computed({
  get: () => String(Math.round(prefs.staleAfterMs / 1000)),
  set: (v: string) => {
    prefs.staleAfterMs = Number(v) * 1000
  },
})
const staleOptions = computed(() => {
  const cur = String(Math.round(prefs.staleAfterMs / 1000))
  return STALE_OPTIONS.some((o) => o.value === cur)
    ? STALE_OPTIONS
    : [...STALE_OPTIONS, { value: cur, label: `${cur} seconds` }].sort(
        (a, b) => Number(a.value) - Number(b.value),
      )
})
</script>

<template>
  <Card
    title="Agent runs"
    icon="bot"
    :subtitle="`${rows.length} active${staleCount ? ` · ${staleCount} quiet` : ''}`"
    flush
  >
    <template #actions>
      <SelectField
        v-model="staleSeconds"
        label="Warn when silent for"
        :options="staleOptions"
        class="stale-pref"
        hint=""
      />
    </template>
    <div class="body" data-testid="runs-panel">
      <div v-if="pending" class="sk">
        <Skeleton :height="96" /><Skeleton :height="96" /><Skeleton :height="96" />
      </div>
      <ErrorState
        v-else-if="board.load.runs.status === 'error' && rows.length === 0"
        what="agent runs"
        :error="board.load.runs.error"
        compact
        @retry="board.refreshAll()"
      />
      <EmptyState
        v-else-if="rows.length === 0"
        icon="bot"
        compact
        title="No agent is running"
        description="When an agent starts a run it appears here with its task, heartbeat and leases."
      />
      <ul v-else class="list" role="list">
        <li
          v-for="r in rows"
          :key="r.run.run_id"
          class="run"
          :class="{ stale: r.health.liveness === 'stale' }"
          :data-run="r.run.run_id"
        >
          <div class="top">
            <IdentityLabel :identity="r.run.identity" :size="22" />
            <Badge v-if="r.health.liveness === 'stale'" tone="warn" icon="alert" size="sm"
              >Quiet</Badge
            >
            <Badge v-else tone="ok" dot size="sm">Running</Badge>
          </div>
          <p class="task">
            <Icon name="task" :size="14" />
            <span v-if="r.task">{{ r.task.title }}</span>
            <span v-else class="faint">no task attached</span>
          </p>
          <p class="beat" :class="{ warn: r.health.liveness === 'stale' }">
            <Icon :name="r.health.liveness === 'stale' ? 'alert' : 'activity'" :size="14" />
            <span>
              Heartbeat
              <RelativeTime :ms="r.run.last_heartbeat_ms || r.run.started_at_ms" />
              <template v-if="r.health.liveness === 'stale'">
                · silent for {{ formatDuration(r.health.ageMs) }}: it may be stuck
              </template>
            </span>
          </p>
          <ul
            v-if="r.leases.length"
            class="leases"
            role="list"
            :aria-label="`Leases held by ${r.run.identity}`"
          >
            <li v-for="l in r.leases" :key="l.id" class="chip" :class="{ ex: l.exclusive }">
              <Icon :name="l.exclusive ? 'lock' : 'unlock'" :size="12" />
              <span class="mono">{{ l.scope }}</span>
              <span v-if="l.exclusive" class="tok mono">#{{ l.fencing_token }}</span>
            </li>
          </ul>
        </li>
      </ul>
      <p class="foot faint">
        Heartbeats leave no trace in the event log, so this panel re-checks every few seconds.
        <RouterLink :to="`/repos/${repo}/agents`">All runs</RouterLink>
      </p>
    </div>
  </Card>
</template>

<style scoped>
.body {
  min-height: 150px;
}
.sk {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  padding: var(--s-4);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.run {
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 6px;
  border-left: 3px solid transparent;
}
.run.stale {
  border-left-color: var(--warn);
  background: color-mix(in srgb, var(--warn-soft) 55%, transparent);
}
.top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
}
.task,
.beat {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  font-size: var(--fs-sm);
  color: var(--text-2);
}
.task :deep(.icon),
.beat :deep(.icon) {
  margin-top: 2px;
  color: var(--text-3);
}
.beat.warn {
  color: var(--warn);
  font-weight: 560;
}
.beat.warn :deep(.icon) {
  color: var(--warn);
}
.leases {
  list-style: none;
  margin: 2px 0 0;
  padding: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border-radius: var(--r-sm);
  border: 1px solid var(--border);
  background: var(--surface-2);
  font-size: var(--fs-xs);
  color: var(--text-2);
}
.chip.ex {
  border-color: var(--warn-line);
  background: var(--warn-soft);
  color: var(--warn);
}
.tok {
  font-weight: 700;
}
.foot {
  padding: var(--s-2) var(--s-4) var(--s-3);
  font-size: var(--fs-xs);
}
.stale-pref {
  width: 9.5rem;
}
.stale-pref :deep(label) {
  font-size: var(--fs-xs);
  font-weight: 500;
  color: var(--text-3);
}
.stale-pref :deep(select) {
  min-height: 28px;
  font-size: var(--fs-sm);
}
</style>
