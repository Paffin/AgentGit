<script setup lang="ts">
import { computed } from 'vue'
import { useServerNow } from '@/composables/useNow'
import { runHealth } from '@/lib/runs'
import { shortId } from '@/lib/format'
import { useBoard } from '@/stores/board'
import { usePrefs } from '@/stores/prefs'
import SelectField from '@/ui/SelectField.vue'
import Skeleton from '@/ui/Skeleton.vue'

const board = useBoard()
const prefs = usePrefs()
const now = useServerNow()

const current = computed(() => board.streams.find((s) => s.name === board.stream))
const ready = computed(
  () =>
    board.cards.filter(
      (c) =>
        c.proposal.state === 'open' &&
        c.prep?.data &&
        c.prep.data.blockers.length === 0 &&
        c.prep.data.conflicts.length === 0,
    ).length,
)
const blocked = computed(() => board.cards.length - ready.value)
const quiet = computed(
  () =>
    board.activeRuns.filter((r) => runHealth(r, now.value, prefs.staleAfterMs).liveness === 'stale')
      .length,
)
const exclusive = computed(() => board.activeLeases.filter((l) => l.exclusive).length)
const waiting = computed(() => board.queueEntries.filter((q) => q.state === 'queued').length)
const counts = computed(() => board.taskLayout.counts)
const loading = computed(
  () => board.load.streams.status !== 'ready' && board.load.proposals.status !== 'ready',
)
const streamOptions = computed(() => board.streams.map((s) => ({ value: s.name, label: s.name })))
const stream = computed({
  get: () => board.stream,
  set: (v: string) => void board.selectStream(v),
})
</script>

<template>
  <section class="summary" aria-label="Summary" data-testid="board-summary">
    <div class="tile wide">
      <template v-if="loading"
        ><Skeleton :height="12" width="60%" /><Skeleton :height="22" width="80%"
      /></template>
      <template v-else>
        <SelectField
          v-if="board.streams.length > 1"
          v-model="stream"
          label="Stream"
          :options="streamOptions"
          class="sel"
        />
        <p v-else class="k">Stream</p>
        <p class="v mono">
          {{ board.stream || '–' }}
          <span class="head">{{ current?.head ? shortId(current.head, 10) : 'empty' }}</span>
        </p>
        <p class="s">
          version {{ current?.version ?? 0 }} · policy v{{ current?.policy_version ?? 0 }}
        </p>
      </template>
    </div>
    <div class="tile">
      <p class="k">Open proposals</p>
      <p class="v">{{ board.cards.length }}</p>
      <p class="s">{{ ready }} ready · {{ blocked }} held back</p>
    </div>
    <div class="tile">
      <p class="k">Agents running</p>
      <p class="v">{{ board.activeRuns.length }}</p>
      <p class="s" :class="{ warn: quiet > 0 }">
        {{ quiet > 0 ? `${quiet} quiet` : 'all heartbeating' }}
      </p>
    </div>
    <div class="tile">
      <p class="k">Active leases</p>
      <p class="v">{{ board.activeLeases.length }}</p>
      <p class="s">{{ exclusive }} exclusive · {{ board.activeLeases.length - exclusive }} soft</p>
    </div>
    <div class="tile">
      <p class="k">Tasks</p>
      <p class="v">{{ counts.ready + counts.in_progress + counts.blocked }}</p>
      <p class="s">{{ counts.in_progress }} in progress · {{ counts.ready }} ready</p>
    </div>
    <div class="tile">
      <p class="k">Queue</p>
      <p class="v">{{ waiting }}</p>
      <p class="s">waiting to integrate</p>
    </div>
  </section>
</template>

<style scoped>
.summary {
  display: grid;
  grid-template-columns: minmax(220px, 1.4fr) repeat(5, minmax(120px, 1fr));
  gap: var(--s-3);
}
.tile {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-3) var(--s-4);
  min-height: 86px;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 2px;
  box-shadow: var(--shadow-1);
}
.k {
  font-size: var(--fs-xs);
  font-weight: 620;
  letter-spacing: 0.03em;
  text-transform: uppercase;
  color: var(--text-3);
}
.v {
  font-size: var(--fs-2xl);
  font-weight: 680;
  line-height: 1.15;
  font-variant-numeric: tabular-nums;
}
.v.mono {
  font-size: var(--fs-xl);
  display: flex;
  align-items: baseline;
  gap: var(--s-2);
  flex-wrap: wrap;
}
.head {
  font-size: var(--fs-sm);
  font-weight: 500;
  color: var(--text-2);
}
.s {
  font-size: var(--fs-sm);
  color: var(--text-2);
}
.s.warn {
  color: var(--warn);
  font-weight: 600;
}
.sel :deep(label) {
  font-size: var(--fs-xs);
  font-weight: 620;
  text-transform: uppercase;
  letter-spacing: 0.03em;
  color: var(--text-3);
}
.sel :deep(select) {
  min-height: 28px;
}
@media (max-width: 1180px) {
  .summary {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
  .wide {
    grid-column: span 3;
  }
}
@media (max-width: 720px) {
  .summary {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .wide {
    grid-column: span 2;
  }
}
</style>
