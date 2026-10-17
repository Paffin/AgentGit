<script setup lang="ts">
import Icon from './Icon.vue'
import type { IconName } from './icons'

withDefaults(
  defineProps<{
    tone?: 'neutral' | 'ok' | 'warn' | 'bad' | 'info' | 'agent' | 'service' | 'accent'
    icon?: IconName
    size?: 'sm' | 'md'
    /** A small coloured dot before the label (for status lists). */
    dot?: boolean
    mono?: boolean
    outline?: boolean
  }>(),
  { tone: 'neutral', icon: undefined, size: 'md', dot: false, mono: false, outline: false },
)
</script>

<template>
  <span class="badge" :class="[`t-${tone}`, `s-${size}`, { mono, outline }]">
    <span v-if="dot" class="dot" aria-hidden="true" />
    <Icon v-if="icon" :name="icon" :size="size === 'sm' ? 12 : 14" />
    <slot />
  </span>
</template>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border: 1px solid var(--b-line);
  background: var(--b-bg);
  color: var(--b-fg);
  border-radius: var(--r-pill);
  font-weight: 560;
  line-height: 1;
  white-space: nowrap;
  vertical-align: middle;
}
.s-md {
  height: 22px;
  padding: 0 9px;
  font-size: var(--fs-xs);
}
.s-sm {
  height: 18px;
  padding: 0 7px;
  font-size: 0.6875rem;
}
.mono {
  font-family: var(--font-mono);
  font-weight: 500;
}
.outline {
  background: transparent;
}
.dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}
.t-neutral {
  --b-bg: var(--surface-2);
  --b-fg: var(--text-2);
  --b-line: var(--border);
}
.t-ok {
  --b-bg: var(--ok-soft);
  --b-fg: var(--ok);
  --b-line: var(--ok-line);
}
.t-warn {
  --b-bg: var(--warn-soft);
  --b-fg: var(--warn);
  --b-line: var(--warn-line);
}
.t-bad {
  --b-bg: var(--bad-soft);
  --b-fg: var(--bad);
  --b-line: var(--bad-line);
}
.t-info {
  --b-bg: var(--info-soft);
  --b-fg: var(--info);
  --b-line: var(--info-line);
}
.t-agent {
  --b-bg: var(--agent-soft);
  --b-fg: var(--agent);
  --b-line: var(--agent-line);
}
.t-service {
  --b-bg: var(--service-soft);
  --b-fg: var(--service);
  --b-line: var(--service-line);
}
.t-accent {
  --b-bg: var(--accent-soft);
  --b-fg: var(--accent-text);
  --b-line: color-mix(in srgb, var(--accent) 35%, transparent);
}
</style>
