<script setup lang="ts">
import { computed } from 'vue'
import { ICONS } from './icons'
import type { IconName } from './icons'

const props = withDefaults(
  defineProps<{
    name: IconName
    size?: number
    /** Gives the icon an accessible name. Without it the icon is decorative and hidden from assistive tech. */
    label?: string
    filled?: boolean
  }>(),
  { size: 16, label: undefined, filled: false },
)

const path = computed(() => ICONS[props.name])
</script>

<template>
  <svg
    class="icon"
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.75"
    stroke-linecap="round"
    stroke-linejoin="round"
    :role="label ? 'img' : undefined"
    :aria-label="label"
    :aria-hidden="label ? undefined : 'true'"
    focusable="false"
  >
    <path :d="path" :fill="filled ? 'currentColor' : 'none'" />
  </svg>
</template>

<style scoped>
.icon {
  flex: none;
  display: inline-block;
  vertical-align: -0.2em;
}
</style>
