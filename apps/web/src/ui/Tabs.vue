<script setup lang="ts" generic="T extends string">
import { nextTick, ref, useId } from 'vue'
import Icon from './Icon.vue'
import type { TabItem } from './types'

const props = defineProps<{
  modelValue: T
  tabs: TabItem<T>[]
  label: string
}>()

const emit = defineEmits<{ 'update:modelValue': [id: T] }>()

const uid = useId()
const list = ref<HTMLElement | null>(null)
const tabId = (id: string) => `${uid}-tab-${id}`
const panelId = (id: string) => `${uid}-panel-${id}`

function select(id: T) {
  emit('update:modelValue', id)
}

async function onKey(e: KeyboardEvent, index: number) {
  const enabled = props.tabs.filter((t) => !t.disabled)
  const current = enabled.findIndex((t) => t.id === props.tabs[index]?.id)
  let next = -1
  if (e.key === 'ArrowRight') next = (current + 1) % enabled.length
  else if (e.key === 'ArrowLeft') next = (current - 1 + enabled.length) % enabled.length
  else if (e.key === 'Home') next = 0
  else if (e.key === 'End') next = enabled.length - 1
  if (next < 0) return
  e.preventDefault()
  const target = enabled[next]
  if (!target) return
  select(target.id)
  await nextTick()
  list.value?.querySelector<HTMLElement>(`#${CSS.escape(tabId(target.id))}`)?.focus()
}
</script>

<template>
  <div class="tabs">
    <div ref="list" class="list" role="tablist" :aria-label="label">
      <button
        v-for="(t, i) in tabs"
        :id="tabId(t.id)"
        :key="t.id"
        type="button"
        role="tab"
        class="tab"
        :class="{ active: t.id === modelValue }"
        :aria-selected="t.id === modelValue"
        :aria-controls="panelId(t.id)"
        :tabindex="t.id === modelValue ? 0 : -1"
        :disabled="t.disabled"
        @click="select(t.id)"
        @keydown="onKey($event, i)"
      >
        <Icon v-if="t.icon" :name="t.icon" :size="15" />
        <span>{{ t.label }}</span>
        <span v-if="t.count !== undefined && t.count !== ''" class="count">{{ t.count }}</span>
      </button>
    </div>
    <div
      :id="panelId(modelValue)"
      class="panel"
      role="tabpanel"
      :aria-labelledby="tabId(modelValue)"
      tabindex="0"
    >
      <slot :name="modelValue" />
      <slot :active="modelValue" />
    </div>
  </div>
</template>

<style scoped>
.list {
  display: flex;
  gap: var(--s-1);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  scrollbar-width: none;
}
.tab {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0 var(--s-3);
  height: 38px;
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
  color: var(--text-2);
  font-weight: 560;
  white-space: nowrap;
  border-radius: 0;
  transition: color var(--dur-1) var(--ease);
}
.tab:hover:not(:disabled) {
  color: var(--text);
}
.tab.active {
  color: var(--text);
  border-bottom-color: var(--accent);
}
.tab:disabled {
  opacity: 0.5;
}
.count {
  min-width: 18px;
  padding: 1px 6px;
  border-radius: var(--r-pill);
  background: var(--surface-3);
  color: var(--text-2);
  font-size: var(--fs-xs);
  font-weight: 600;
  text-align: center;
}
.panel {
  padding-top: var(--s-4);
}
.panel:focus-visible {
  outline-offset: -2px;
}
</style>
