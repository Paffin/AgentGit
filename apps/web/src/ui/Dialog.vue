<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import { focusableIn, trapTab } from '@/composables/useFocusTrap'
import Button from './Button.vue'
import { modalLayer } from '@/composables/modalLayer'

const props = withDefaults(
  defineProps<{
    modelValue: boolean
    title: string
    description?: string
    size?: 'sm' | 'md' | 'lg' | 'xl'
    /** Escape and a click on the backdrop close the dialog. Turn off while a request is in flight. */
    dismissable?: boolean
    /** An alert dialog interrupts: used for destructive confirmations. */
    alert?: boolean
  }>(),
  { description: undefined, size: 'md', dismissable: true, alert: false },
)

const emit = defineEmits<{ 'update:modelValue': [open: boolean]; close: [] }>()

const titleId = useId()
const descId = useId()
const panel = ref<HTMLElement | null>(null)
let returnTo: HTMLElement | null = null
const layer = modalLayer()

function close() {
  if (!props.dismissable || !layer.top.value) return
  emit('update:modelValue', false)
  emit('close')
}

function onKeydown(e: KeyboardEvent) {
  if (!layer.top.value) return
  if (e.key === 'Escape' && props.dismissable) {
    e.stopPropagation()
    close()
    return
  }
  if (panel.value) trapTab(e, panel.value)
}

function lock() {
  returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null
  layer.activate()
}
function unlock() {
  layer.deactivate()
  const target = returnTo
  returnTo = null
  // Restore focus after the DOM settled; the opener may have been re-rendered.
  void nextTick(() => {
    if (target && document.contains(target) && !target.closest('[inert]')) target.focus()
  })
}

watch(
  () => props.modelValue,
  async (open) => {
    if (open) {
      lock()
      await nextTick()
      const el = panel.value
      if (!el || !layer.top.value) return
      const preferred = el.querySelector<HTMLElement>('[data-autofocus]')
      const first = preferred ?? focusableIn(el).find((n) => !n.hasAttribute('data-dialog-close'))
      ;(first ?? el).focus()
    } else {
      unlock()
    }
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  unlock()
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="scrim"
      :style="{ zIndex: layer.zIndex.value }"
      :inert="!layer.top.value"
      :aria-hidden="!layer.top.value || undefined"
      @mousedown.self="close"
    >
      <div
        ref="panel"
        class="panel"
        :class="`size-${size}`"
        :role="alert ? 'alertdialog' : 'dialog'"
        aria-modal="true"
        :aria-labelledby="titleId"
        :aria-describedby="description ? descId : undefined"
        tabindex="-1"
        @keydown="onKeydown"
      >
        <header class="head">
          <div class="titles">
            <h2 :id="titleId" class="title">{{ title }}</h2>
            <p v-if="description" :id="descId" class="desc">{{ description }}</p>
          </div>
          <Button
            v-if="dismissable"
            variant="ghost"
            size="sm"
            icon="x"
            label="Close dialog"
            data-dialog-close
            @click="close"
          />
        </header>
        <div class="content"><slot /></div>
        <footer v-if="$slots.footer" class="foot"><slot name="footer" /></footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.scrim {
  position: fixed;
  inset: 0;
  z-index: var(--z-dialog);
  display: grid;
  place-items: start center;
  padding: clamp(16px, 8vh, 96px) var(--s-4) var(--s-4);
  background: var(--overlay);
  overflow-y: auto;
  animation: fade var(--dur-2) var(--ease);
}
.panel {
  width: 100%;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-3);
  display: flex;
  flex-direction: column;
  max-height: calc(100vh - 2 * clamp(16px, 8vh, 96px));
  animation: rise var(--dur-2) var(--ease);
}
.panel:focus {
  outline: none;
}
.size-sm {
  max-width: 420px;
}
.size-md {
  max-width: 560px;
}
.size-lg {
  max-width: 760px;
}
.size-xl {
  max-width: 1040px;
}
.head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--s-3);
  padding: var(--s-4) var(--s-4) var(--s-3);
}
.title {
  font-size: var(--fs-lg);
}
.desc {
  color: var(--text-2);
  margin-top: var(--s-1);
  font-size: var(--fs-md);
}
.content {
  padding: 0 var(--s-4) var(--s-4);
  overflow-y: auto;
  min-height: 0;
}
.foot {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: var(--s-2);
  padding: var(--s-3) var(--s-4);
  border-top: 1px solid var(--border);
  background: var(--surface-2);
  border-radius: 0 0 var(--r-lg) var(--r-lg);
}
@keyframes fade {
  from {
    opacity: 0;
  }
}
@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(8px) scale(0.99);
  }
}
</style>
