<script setup lang="ts">
import Icon from './Icon.vue'
import type { IconName } from './icons'

withDefaults(
  defineProps<{
    title: string
    description?: string
    icon?: IconName
    /** Compact variant for panels on a dashboard. */
    compact?: boolean
  }>(),
  { description: undefined, icon: 'circle', compact: false },
)
</script>

<template>
  <div class="empty" :class="{ compact }">
    <span class="glyph"><Icon :name="icon" :size="compact ? 18 : 22" /></span>
    <p class="title">{{ title }}</p>
    <p v-if="description" class="desc">{{ description }}</p>
    <div v-if="$slots.default" class="actions"><slot /></div>
  </div>
</template>

<style scoped>
.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: var(--s-2);
  padding: var(--s-6) var(--s-4);
  color: var(--text-2);
}
.compact {
  padding: var(--s-4) var(--s-3);
}
.glyph {
  display: grid;
  place-items: center;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--surface-2);
  color: var(--text-3);
  border: 1px dashed var(--border-strong);
}
.compact .glyph {
  width: 34px;
  height: 34px;
}
.title {
  font-weight: 620;
  color: var(--text);
}
.desc {
  max-width: 46ch;
  font-size: var(--fs-sm);
}
.actions {
  margin-top: var(--s-2);
  display: flex;
  gap: var(--s-2);
}
</style>
