<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, useId, watch } from 'vue'

const props = withDefaults(
  defineProps<{
    /** Plain text. For anything richer use the `content` slot. */
    text?: string
    placement?: 'top' | 'bottom'
    delay?: number
    /** Keep the wrapper out of the tab order even when it contains no focusable child. */
    inline?: boolean
  }>(),
  { text: undefined, placement: 'top', delay: 250, inline: false },
)

const id = useId()
const open = ref(false)
const root = ref<HTMLElement | null>(null)
let timer: ReturnType<typeof setTimeout> | undefined

function show(immediate = false) {
  clearTimeout(timer)
  if (immediate || props.delay <= 0) open.value = true
  else timer = setTimeout(() => (open.value = true), props.delay)
}
function hide() {
  clearTimeout(timer)
  open.value = false
}
function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && open.value) {
    hide()
  }
}
// While it is showing, Escape dismisses it wherever focus or the pointer is (WCAG 1.4.13).
watch(open, (o) => {
  if (o) document.addEventListener('keydown', onKey)
  else document.removeEventListener('keydown', onKey)
})

onMounted(() => {
  const el = root.value
  if (!el) return
  const focusable = el.querySelector<HTMLElement>(
    'button, a[href], input, select, textarea, [tabindex]',
  )
  if (focusable) {
    focusable.setAttribute('aria-describedby', id)
  } else if (!props.inline) {
    el.setAttribute('tabindex', '0')
    el.setAttribute('aria-describedby', id)
  }
})
onBeforeUnmount(() => {
  clearTimeout(timer)
  document.removeEventListener('keydown', onKey)
})
</script>

<template>
  <span
    ref="root"
    class="tt"
    @mouseenter="show()"
    @mouseleave="hide"
    @focusin="show(true)"
    @focusout="hide"
    @keydown="onKey"
  >
    <slot />
    <span v-show="open" :id="id" role="tooltip" class="bubble" :class="placement">
      <slot name="content">{{ text }}</slot>
    </span>
  </span>
</template>

<style scoped>
.tt {
  position: relative;
  display: inline-flex;
  max-width: 100%;
}
.bubble {
  position: absolute;
  left: 50%;
  transform: translateX(-50%);
  z-index: var(--z-tooltip);
  max-width: 280px;
  width: max-content;
  padding: 6px 9px;
  border-radius: var(--r-sm);
  background: var(--text);
  color: var(--bg);
  font-size: var(--fs-xs);
  font-weight: 500;
  line-height: 1.35;
  box-shadow: var(--shadow-2);
  pointer-events: none;
  white-space: normal;
  text-align: left;
}
.top {
  bottom: calc(100% + 6px);
}
.bottom {
  top: calc(100% + 6px);
}
</style>
