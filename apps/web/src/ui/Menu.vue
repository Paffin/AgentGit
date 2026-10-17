<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import Icon from './Icon.vue'
import type { MenuItem } from './types'

defineProps<{ label: string; items: readonly MenuItem[]; align?: 'start' | 'end' }>()
const emit = defineEmits<{ select: [id: string] }>()

const open = ref(false)
const root = ref<HTMLElement | null>(null)
const menuId = useId()

function menuItems(): HTMLElement[] {
  return Array.from(
    root.value?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])') ??
      [],
  )
}
async function toggle(focusFirst = false) {
  open.value = !open.value
  if (open.value) {
    await nextTick()
    if (focusFirst) menuItems()[0]?.focus()
  }
}
function close(returnFocus = true) {
  if (!open.value) return
  open.value = false
  if (returnFocus) root.value?.querySelector<HTMLElement>('[aria-haspopup]')?.focus()
}
function onOutside(e: MouseEvent) {
  if (root.value && !root.value.contains(e.target as Node)) close(false)
}
function onKey(e: KeyboardEvent) {
  if (!open.value) return
  const list = menuItems()
  const i = list.indexOf(document.activeElement as HTMLElement)
  if (e.key === 'Escape') {
    e.preventDefault()
    close()
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    list[(i + 1) % list.length]?.focus()
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    list[(i - 1 + list.length) % list.length]?.focus()
  } else if (e.key === 'Tab') {
    close(false)
  }
}
function choose(id: string) {
  emit('select', id)
  close()
}

watch(open, (o) => {
  if (o) document.addEventListener('mousedown', onOutside)
  else document.removeEventListener('mousedown', onOutside)
})
onBeforeUnmount(() => document.removeEventListener('mousedown', onOutside))
</script>

<template>
  <div ref="root" class="menu" @keydown="onKey">
    <div class="trigger" @keydown.down.prevent="toggle(true)">
      <slot name="trigger" :open="open" :toggle="() => toggle(false)" :menu-id="menuId" />
    </div>
    <div
      v-if="open"
      :id="menuId"
      class="pop"
      :class="`a-${align ?? 'end'}`"
      role="menu"
      :aria-label="label"
    >
      <button
        v-for="it in items"
        :key="it.id"
        type="button"
        role="menuitem"
        class="item"
        :class="{ danger: it.danger }"
        :aria-disabled="it.disabled ? 'true' : undefined"
        @click="!it.disabled && choose(it.id)"
      >
        <Icon v-if="it.icon" :name="it.icon" :size="15" />
        {{ it.label }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.menu {
  position: relative;
  display: inline-block;
}
.pop {
  position: absolute;
  top: calc(100% + 6px);
  z-index: var(--z-dropdown);
  min-width: 200px;
  padding: var(--s-1);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  box-shadow: var(--shadow-2);
}
.a-end {
  right: 0;
}
.a-start {
  left: 0;
}
.item {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  width: 100%;
  padding: 7px var(--s-3);
  background: none;
  border: none;
  border-radius: var(--r-sm);
  text-align: left;
  color: var(--text);
}
.item:hover,
.item:focus-visible {
  background: var(--surface-3);
}
.item.danger {
  color: var(--bad);
}
.item[aria-disabled='true'] {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
