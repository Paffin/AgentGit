<script setup lang="ts">
import { computed } from 'vue'
import type { StateInfo, StateTone } from '@/lib/fourstate'
import Icon from './Icon.vue'
import type { IconName } from './icons'
import Tooltip from './Tooltip.vue'
import { termLabel } from '@/lib/glossary'

const props = withDefaults(defineProps<{ state: StateInfo; compact?: boolean }>(), {
  compact: false,
})

// Shape carries the meaning as well as colour: check, hourglass-ish clock, alert, cross, dash, question.
const ICON: Record<StateTone, IconName> = {
  ok: 'check',
  pending: 'clock',
  blocked: 'alert',
  failed: 'x',
  na: 'minus',
  unknown: 'circle',
}
const icon = computed(() => ICON[props.state.tone])
</script>

<template>
  <Tooltip :text="state.summary" inline>
    <span class="state" :class="`t-${state.tone}`" :data-state="state.key" :data-tone="state.tone">
      <Icon :name="icon" :size="compact ? 12 : 14" />
      <span class="name">{{ termLabel(state.key) }}</span>
      <span class="value">{{ state.value }}</span>
      <span class="sr-only">. {{ state.summary }}</span>
    </span>
  </Tooltip>
</template>

<style scoped>
.state {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 9px 0 7px;
  border-radius: var(--r-sm);
  border: 1px solid var(--s-line);
  background: var(--s-bg);
  color: var(--s-fg);
  font-size: var(--fs-xs);
  white-space: nowrap;
}
.name {
  font-weight: 650;
}
.value {
  font-weight: 500;
  padding-left: 6px;
  border-left: 1px solid var(--s-line);
}
.t-ok {
  --s-fg: var(--ok);
  --s-bg: var(--ok-soft);
  --s-line: var(--ok-line);
}
.t-pending {
  --s-fg: var(--info);
  --s-bg: var(--info-soft);
  --s-line: var(--info-line);
}
.t-blocked {
  --s-fg: var(--warn);
  --s-bg: var(--warn-soft);
  --s-line: var(--warn-line);
}
.t-failed {
  --s-fg: var(--bad);
  --s-bg: var(--bad-soft);
  --s-line: var(--bad-line);
}
.t-na,
.t-unknown {
  --s-fg: var(--text-2);
  --s-bg: var(--surface-2);
  --s-line: var(--border);
}
.t-unknown {
  border-style: dashed;
}
</style>
