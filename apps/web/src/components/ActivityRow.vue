<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { EventView } from '@/lib/events'
import { shortId } from '@/lib/format'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import Icon from '@/ui/Icon.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{ event: EventView; repo: string; fresh?: boolean }>()

const link = computed(() => {
  const e = props.event
  if (e.objectKind === 'proposal' && e.object.startsWith('prop_')) {
    return `/repos/${props.repo}/proposals/${encodeURIComponent(e.object)}`
  }
  return ''
})
const objectText = computed(() => {
  const o = props.event.object
  return o.startsWith('prop_') ||
    o.startsWith('run_') ||
    o.startsWith('lease_') ||
    o.startsWith('job_')
    ? shortId(o, 8)
    : o
})
</script>

<template>
  <div
    class="row"
    :class="[`t-${event.tone}`, { fresh, minor: event.minor }]"
    :data-topic="event.topic"
    :data-seq="event.seq"
  >
    <span class="mark" aria-hidden="true"><Icon :name="event.icon" :size="13" /></span>
    <div class="text">
      <p class="line">
        <IdentityLabel v-if="event.actor" :identity="event.actor" :size="16" hide-run />
        <span class="verb">{{ event.verb }}</span>
        <RouterLink v-if="link" :to="link" class="obj mono">{{ objectText }}</RouterLink>
        <code v-else-if="objectText" class="obj mono">{{ objectText }}</code>
        <span v-if="event.note" class="note faint">{{ event.note }}</span>
      </p>
    </div>
    <RelativeTime :ms="event.timestamp" class="when faint" />
  </div>
</template>

<style scoped>
.row {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  height: 100%;
  padding: 0 var(--s-4);
  border-bottom: 1px solid var(--border);
  font-size: var(--fs-sm);
  --c: var(--text-3);
  --c-bg: var(--surface-2);
}
.row.minor {
  opacity: 0.75;
}
.fresh {
  animation: arrive 2.6s var(--ease);
}
@keyframes arrive {
  from {
    background: var(--accent-soft);
  }
  to {
    background: transparent;
  }
}
.mark {
  flex: none;
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: var(--c-bg);
  color: var(--c);
}
.text {
  flex: 1;
  min-width: 0;
}
.line {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 0 6px;
  max-height: 40px;
  overflow: hidden;
  line-height: 1.35;
}
.verb {
  color: var(--text-2);
}
.obj {
  font-size: var(--fs-xs);
  padding: 0 5px;
  border-radius: 4px;
  background: var(--surface-3);
  color: var(--text);
}
a.obj {
  color: var(--accent-text);
}
.when {
  flex: none;
  font-size: var(--fs-xs);
}
.t-ok {
  --c: var(--ok);
  --c-bg: var(--ok-soft);
}
.t-warn {
  --c: var(--warn);
  --c-bg: var(--warn-soft);
}
.t-bad {
  --c: var(--bad);
  --c-bg: var(--bad-soft);
}
.t-info {
  --c: var(--info);
  --c-bg: var(--info-soft);
}
.t-agent {
  --c: var(--agent);
  --c-bg: var(--agent-soft);
}
@media (prefers-reduced-motion: reduce) {
  .fresh {
    animation: none;
    background: var(--accent-soft);
  }
}
</style>
