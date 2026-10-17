<script setup lang="ts" generic="Item extends { id: string | number }">
import Icon from './Icon.vue'
import type { IconName } from './icons'

defineProps<{
  items: readonly Item[]
  /** Accessible name of the list. */
  label: string
  icon?: (item: Item) => IconName
  tone?: (item: Item) => 'neutral' | 'ok' | 'warn' | 'bad' | 'info' | 'agent'
  dim?: (item: Item) => boolean
}>()
</script>

<template>
  <ol class="timeline" :aria-label="label" role="list">
    <li
      v-for="item in items"
      :key="item.id"
      class="item"
      :class="[`t-${tone?.(item) ?? 'neutral'}`, { dim: dim?.(item) ?? false }]"
    >
      <span class="marker" aria-hidden="true"
        ><Icon :name="icon?.(item) ?? 'dot'" :size="13"
      /></span>
      <div class="content"><slot :item="item" /></div>
    </li>
  </ol>
</template>

<style scoped>
.timeline {
  list-style: none;
  margin: 0;
  padding: 0;
}
.item {
  position: relative;
  display: flex;
  gap: var(--s-3);
  padding-bottom: var(--s-3);
  --c: var(--text-3);
  --c-bg: var(--surface-2);
  --c-line: var(--border);
}
.item:last-child {
  padding-bottom: 0;
}
.item::before {
  content: '';
  position: absolute;
  left: 11px;
  top: 24px;
  bottom: 0;
  width: 1px;
  background: var(--border);
}
.item:last-child::before {
  display: none;
}
.marker {
  flex: none;
  display: grid;
  place-items: center;
  width: 23px;
  height: 23px;
  border-radius: 50%;
  background: var(--c-bg);
  border: 1px solid var(--c-line);
  color: var(--c);
}
.content {
  flex: 1;
  min-width: 0;
  padding-top: 1px;
}
.dim .content {
  opacity: 0.8;
}
.t-ok {
  --c: var(--ok);
  --c-bg: var(--ok-soft);
  --c-line: var(--ok-line);
}
.t-warn {
  --c: var(--warn);
  --c-bg: var(--warn-soft);
  --c-line: var(--warn-line);
}
.t-bad {
  --c: var(--bad);
  --c-bg: var(--bad-soft);
  --c-line: var(--bad-line);
}
.t-info {
  --c: var(--info);
  --c-bg: var(--info-soft);
  --c-line: var(--info-line);
}
.t-agent {
  --c: var(--agent);
  --c-bg: var(--agent-soft);
  --c-line: var(--agent-line);
}
</style>
