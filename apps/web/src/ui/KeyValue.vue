<script setup lang="ts">
import type { KvItem } from './types'

withDefaults(defineProps<{ items: readonly KvItem[]; columns?: 1 | 2; labelWidth?: string }>(), {
  columns: 1,
  labelWidth: '9.5rem',
})
</script>

<template>
  <dl class="kv" :class="`c-${columns}`" :style="{ '--lw': labelWidth }">
    <div v-for="it in items" :key="it.key" class="pair">
      <dt>{{ it.label }}</dt>
      <dd :class="{ mono: it.mono }">
        <slot :name="`value-${it.key}`" :item="it">{{
          it.value === undefined || it.value === null || it.value === '' ? '–' : it.value
        }}</slot>
      </dd>
    </div>
  </dl>
</template>

<style scoped>
.kv {
  display: grid;
  gap: var(--s-2) var(--s-5);
  margin: 0;
}
.c-2 {
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 22rem), 1fr));
}
.pair {
  display: grid;
  grid-template-columns: minmax(0, var(--lw)) minmax(0, 1fr);
  gap: var(--s-3);
  align-items: baseline;
}
dt {
  color: var(--text-3);
  font-size: var(--fs-sm);
}
dd {
  margin: 0;
  min-width: 0;
  overflow-wrap: anywhere;
}
dd.mono {
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
}
</style>
