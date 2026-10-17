<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { RouteLocationRaw } from 'vue-router'
import Icon from './Icon.vue'
import Spinner from './Spinner.vue'
import type { IconName } from './icons'

const props = withDefaults(
  defineProps<{
    variant?: 'primary' | 'secondary' | 'ghost' | 'danger'
    size?: 'sm' | 'md'
    icon?: IconName
    iconRight?: IconName
    loading?: boolean
    disabled?: boolean
    /** Why the button is unavailable. Shown as a tooltip and announced; the button stays focusable. */
    disabledReason?: string
    type?: 'button' | 'submit' | 'reset'
    to?: RouteLocationRaw
    block?: boolean
    /** Icon-only button: the label is required for assistive tech. */
    label?: string
  }>(),
  {
    variant: 'secondary',
    size: 'md',
    icon: undefined,
    iconRight: undefined,
    loading: false,
    disabled: false,
    disabledReason: undefined,
    type: 'button',
    to: undefined,
    block: false,
    label: undefined,
  },
)

const emit = defineEmits<{ click: [event: MouseEvent] }>()

const inert = computed(() => props.disabled || props.loading)
const reasonId = `btn-reason-${Math.random().toString(36).slice(2, 9)}`

function onClick(e: MouseEvent) {
  if (inert.value) {
    e.preventDefault()
    e.stopPropagation()
    return
  }
  emit('click', e)
}
</script>

<template>
  <RouterLink
    v-if="to && !inert"
    :to="to"
    class="btn"
    :class="[`v-${variant}`, `s-${size}`, { block, 'icon-only': !$slots.default }]"
    :aria-label="label"
  >
    <Icon v-if="icon" :name="icon" :size="size === 'sm' ? 14 : 16" />
    <slot />
    <Icon v-if="iconRight" :name="iconRight" :size="size === 'sm' ? 14 : 16" />
  </RouterLink>
  <button
    v-else
    class="btn"
    :class="[
      `v-${variant}`,
      `s-${size}`,
      { block, loading, 'is-disabled': inert, 'icon-only': !$slots.default },
    ]"
    :type="type"
    :aria-disabled="inert ? 'true' : undefined"
    :aria-busy="loading ? 'true' : undefined"
    :aria-label="label"
    :aria-describedby="disabled && disabledReason ? reasonId : undefined"
    :title="disabled && disabledReason ? disabledReason : undefined"
    @click="onClick"
  >
    <Spinner v-if="loading" :size="14" />
    <Icon v-else-if="icon" :name="icon" :size="size === 'sm' ? 14 : 16" />
    <span v-if="$slots.default" class="text"><slot /></span>
    <Icon v-if="iconRight" :name="iconRight" :size="size === 'sm' ? 14 : 16" />
    <span v-if="disabled && disabledReason" :id="reasonId" class="sr-only">{{
      disabledReason
    }}</span>
  </button>
</template>

<style scoped>
.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--s-2);
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  font-weight: 560;
  line-height: 1;
  white-space: nowrap;
  text-decoration: none;
  transition:
    background-color var(--dur-1) var(--ease),
    border-color var(--dur-1) var(--ease),
    color var(--dur-1) var(--ease);
  user-select: none;
}
.btn:hover {
  text-decoration: none;
}
.s-md {
  height: 34px;
  padding: 0 var(--s-3);
  font-size: var(--fs-md);
}
.s-sm {
  height: 28px;
  padding: 0 var(--s-2);
  font-size: var(--fs-sm);
  gap: 6px;
}
.icon-only.s-md {
  width: 34px;
  padding: 0;
}
.icon-only.s-sm {
  width: 28px;
  padding: 0;
}
.block {
  width: 100%;
}

.v-primary {
  background: var(--accent);
  color: var(--accent-fg);
}
.v-primary:hover:not(.is-disabled) {
  background: var(--accent-hover);
}
.v-secondary {
  background: var(--surface);
  color: var(--text);
  border-color: var(--border-strong);
}
.v-secondary:hover:not(.is-disabled) {
  background: var(--surface-2);
}
.v-ghost {
  background: transparent;
  color: var(--text-2);
}
.v-ghost:hover:not(.is-disabled) {
  background: var(--surface-3);
  color: var(--text);
}
.v-danger {
  background: var(--surface);
  color: var(--bad);
  border-color: var(--bad-line);
}
.v-danger:hover:not(.is-disabled) {
  background: var(--bad-soft);
}
.is-disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.loading {
  cursor: progress;
}
</style>
