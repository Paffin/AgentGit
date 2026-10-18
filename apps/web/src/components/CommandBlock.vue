<script setup lang="ts">
import { useScrollRegion } from '@/composables/useScrollRegion'

/** Shell commands, one per line, in a box that scrolls sideways (and is reachable by keyboard when it does). */
const props = defineProps<{ lines: string[]; label: string; testid?: string }>()
const { setEl, attrs } = useScrollRegion(() => props.label)
</script>

<template>
  <pre
    :ref="setEl"
    class="cmd"
    v-bind="attrs"
    :data-testid="testid"
  ><code>{{ lines.join('\n') }}</code></pre>
</template>

<style scoped>
.cmd {
  margin: 0;
  padding: var(--s-3) var(--s-4);
  background: var(--surface-2, var(--surface));
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow-x: auto;
  overflow-y: hidden;
  white-space: pre;
  user-select: all;
}
</style>
