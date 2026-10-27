<script setup lang="ts">
import { computed } from 'vue'
import type { Blocker } from '@/api/types'
import { explainAll } from '@/lib/blockers'
import type { FixId, FixPlan } from '@/lib/fixes'
import Button from '@/ui/Button.vue'
import Icon from '@/ui/Icon.vue'

const props = withDefaults(
  defineProps<{
    blockers: readonly Blocker[]
    /** Show the remedy under each blocker. */
    hints?: boolean
    /** Show at most this many; the rest collapses into "and N more". */
    max?: number
    /** What can be done about a blocker (by code). Without it the list only explains. */
    plan?: (code: string) => FixPlan
  }>(),
  { hints: false, max: 99, plan: undefined },
)
const emit = defineEmits<{ fix: [id: FixId, code: string] }>()

const items = computed(() => explainAll(props.blockers))
const shown = computed(() => items.value.slice(0, props.max))
const rest = computed(() => Math.max(0, items.value.length - props.max))
</script>

<template>
  <ul v-if="items.length" class="blockers" role="list" aria-label="What is blocking this">
    <li
      v-for="b in shown"
      :key="b.code + b.message"
      class="b"
      :class="b.severity"
      :data-code="b.code"
    >
      <Icon :name="b.severity === 'refused' ? 'x' : 'alert'" :size="14" class="mark" />
      <div>
        <p class="title" :title="b.message">{{ b.title }}</p>
        <p v-if="hints && b.hint" class="hint">{{ b.hint }}</p>
        <div v-if="plan" class="fix">
          <Button
            v-for="f in plan(b.code).fixes"
            :key="f.id"
            size="sm"
            :data-testid="`fix-${f.id}`"
            @click="emit('fix', f.id, b.code)"
            >{{ f.label }}</Button
          >
          <span v-if="plan(b.code).ask" class="ask" :data-testid="`ask-${b.code}`">{{
            plan(b.code).ask
          }}</span>
        </div>
      </div>
    </li>
    <li v-if="rest" class="more faint">and {{ rest }} more</li>
  </ul>
</template>

<style scoped>
.blockers {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.b {
  display: flex;
  gap: 8px;
  align-items: flex-start;
  font-size: var(--fs-sm);
}
.mark {
  margin-top: 3px;
}
.provide .mark {
  color: var(--warn);
}
.refused .mark {
  color: var(--bad);
}
.title {
  font-weight: 560;
}
.hint {
  color: var(--text-2);
  font-size: var(--fs-xs);
  margin-top: 1px;
}
.fix {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
  margin-top: var(--s-2);
}
.ask {
  color: var(--text-2);
  font-size: var(--fs-xs);
}
.more {
  font-size: var(--fs-xs);
  padding-left: 22px;
}
</style>
