<script setup lang="ts">
import { computed } from 'vue'
import { useServerNow } from '@/composables/useNow'
import { formatCountdown } from '@/lib/format'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import Skeleton from '@/ui/Skeleton.vue'

const board = useBoard()
const now = useServerNow()

const rows = computed(() =>
  board.leaseRows.map(({ lease, overlaps }) => {
    const total = Math.max(1, lease.expires_at_ms - lease.acquired_at_ms)
    const left = lease.expires_at_ms - now.value
    return {
      lease,
      overlaps,
      pct: Math.max(0, Math.min(100, (left / total) * 100)),
      expired: left <= 0,
      low: left > 0 && left / total < 0.2,
      label: formatCountdown(lease.expires_at_ms, now.value),
      segments: lease.scope.split('/'),
    }
  }),
)
const overlapCount = computed(() => rows.value.filter((r) => r.overlaps.length > 0).length)
const exclusive = computed(() => rows.value.filter((r) => r.lease.exclusive).length)
</script>

<template>
  <Card
    title="Lease map"
    icon="lock"
    :subtitle="`${exclusive} exclusive · ${rows.length - exclusive} soft${overlapCount ? ` · ${overlapCount} overlapping` : ''}`"
    flush
  >
    <div class="body" data-testid="lease-map">
      <div v-if="board.load.leases.status === 'loading' && rows.length === 0" class="sk">
        <Skeleton :height="44" /><Skeleton :height="44" />
      </div>
      <ErrorState
        v-else-if="board.load.leases.status === 'error' && rows.length === 0"
        what="leases"
        :error="board.load.leases.error"
        compact
        @retry="board.refreshAll()"
      />
      <EmptyState
        v-else-if="rows.length === 0"
        icon="unlock"
        compact
        title="No active leases"
        description="Soft leases are signals; exclusive leases carry a fencing token and are enforced at integration."
      />
      <ul v-else class="list" role="list">
        <li
          v-for="r in rows"
          :key="r.lease.id"
          class="lease"
          :class="{ overlap: r.overlaps.length > 0, expired: r.expired }"
          :data-lease="r.lease.id"
        >
          <div class="line1">
            <Icon
              :name="r.lease.exclusive ? 'lock' : 'unlock'"
              :size="15"
              class="kind"
              :label="r.lease.exclusive ? 'Exclusive lease' : 'Soft lease'"
            />
            <span class="scope mono">
              <template v-for="(seg, i) in r.segments" :key="i"
                ><span v-if="i > 0" class="sep">/</span
                ><span :class="{ wild: seg.includes('*') }">{{ seg }}</span></template
              >
            </span>
            <Badge v-if="r.lease.exclusive" tone="warn" mono size="sm"
              >token {{ r.lease.fencing_token }}</Badge
            >
            <Badge v-else size="sm">soft</Badge>
          </div>
          <div class="line2">
            <IdentityLabel :identity="r.lease.holder" :size="18" />
            <span class="left" :class="{ low: r.low, gone: r.expired }">{{
              r.expired ? 'expired' : `expires in ${r.label}`
            }}</span>
          </div>
          <div class="bar" aria-hidden="true">
            <span :style="{ width: `${r.pct}%` }" :class="{ low: r.low, gone: r.expired }" />
          </div>
          <p v-if="r.overlaps.length" class="ov">
            <Icon name="alert" :size="13" />
            Overlaps
            <span v-for="o in r.overlaps" :key="o.id" class="mono ovscope">{{ o.scope }}</span>
            <span class="faint"
              >held by {{ r.overlaps.map((o) => o.holder.split('#')[0]).join(', ') }}</span
            >
          </p>
        </li>
      </ul>
    </div>
  </Card>
</template>

<style scoped>
.body {
  min-height: 130px;
}
.sk {
  padding: var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.lease {
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
  border-left: 3px solid transparent;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.lease.overlap {
  border-left-color: var(--warn);
  background: color-mix(in srgb, var(--warn-soft) 50%, transparent);
}
.lease.expired {
  opacity: 0.7;
}
.line1,
.line2 {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  min-width: 0;
}
.line2 {
  justify-content: space-between;
  font-size: var(--fs-sm);
}
.kind {
  color: var(--text-3);
}
.scope {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.sep {
  color: var(--text-3);
  margin: 0 1px;
}
.wild {
  color: var(--agent);
}
.left {
  color: var(--text-2);
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.left.low {
  color: var(--warn);
  font-weight: 600;
}
.left.gone {
  color: var(--text-3);
}
.bar {
  height: 4px;
  border-radius: 2px;
  background: var(--surface-3);
  overflow: hidden;
}
.bar span {
  display: block;
  height: 100%;
  background: var(--info);
  border-radius: 2px;
  transition: width 1s linear;
}
.bar span.low {
  background: var(--warn);
}
.bar span.gone {
  background: var(--text-3);
}
.ov {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  color: var(--warn);
  font-size: var(--fs-xs);
  font-weight: 560;
}
.ovscope {
  padding: 1px 6px;
  border-radius: var(--r-sm);
  background: var(--warn-soft);
  border: 1px solid var(--warn-line);
}
@media (prefers-reduced-motion: reduce) {
  .bar span {
    transition: none;
  }
}
</style>
