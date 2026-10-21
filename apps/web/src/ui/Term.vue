<script setup lang="ts">
import { computed } from 'vue'
import { termLabel, termTip } from '@/lib/glossary'
import type { TermId } from '@/lib/glossary'
import Tooltip from './Tooltip.vue'

/**
 * A plain-language label with the AgentGit word in a tooltip (lib/glossary.ts is the only source).
 * `quiet` is for repeated places (table rows): a native title instead of a tab stop per occurrence.
 */
const props = withDefaults(
  defineProps<{
    id: TermId
    plural?: boolean
    /** Lower-case the first letter (mid-sentence use). */
    lower?: boolean
    quiet?: boolean
  }>(),
  { plural: false, lower: false, quiet: false },
)

const text = computed(() => {
  const t = termLabel(props.id, props.plural)
  return props.lower ? t.charAt(0).toLowerCase() + t.slice(1) : t
})
const tip = computed(() => termTip(props.id))
</script>

<template>
  <span v-if="quiet" class="term" :title="tip" :data-term="id">{{ text }}</span>
  <Tooltip v-else :text="tip" placement="bottom">
    <button type="button" class="term" :data-term="id">{{ text }}</button>
  </Tooltip>
</template>

<style scoped>
.term {
  font: inherit;
  color: inherit;
  background: none;
  border: 0;
  padding: 0;
  text-align: inherit;
  text-decoration: underline dotted var(--text-3);
  text-underline-offset: 3px;
  cursor: help;
}
.term:focus-visible {
  outline: 2px solid var(--focus, var(--accent));
  outline-offset: 2px;
  border-radius: 2px;
}
</style>
