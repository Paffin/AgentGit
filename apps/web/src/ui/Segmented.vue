<script setup lang="ts" generic="V extends string">
import { useId } from 'vue'
import Icon from './Icon.vue'
import type { IconName } from './icons'

defineProps<{
  modelValue: V
  legend: string
  options: readonly { value: V; label: string; icon?: IconName; hint?: string }[]
  hideLegend?: boolean
}>()

const emit = defineEmits<{ 'update:modelValue': [value: V] }>()
const name = useId()
</script>

<template>
  <fieldset class="seg">
    <legend :class="{ 'sr-only': hideLegend }">{{ legend }}</legend>
    <div class="row">
      <label
        v-for="o in options"
        :key="o.value"
        class="opt"
        :class="{ on: o.value === modelValue }"
      >
        <input
          type="radio"
          :name="name"
          :value="o.value"
          :checked="o.value === modelValue"
          @change="emit('update:modelValue', o.value)"
        />
        <Icon v-if="o.icon" :name="o.icon" :size="15" />
        <span>{{ o.label }}</span>
      </label>
    </div>
  </fieldset>
</template>

<style scoped>
.seg {
  border: none;
  padding: 0;
  margin: 0;
  min-width: 0;
}
legend {
  font-weight: 580;
  font-size: var(--fs-sm);
  padding: 0;
  margin-bottom: 6px;
}
.row {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 2px;
  padding: 2px;
  background: var(--surface-3);
  border-radius: var(--r-sm);
}
.opt {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  min-height: var(--touch-min);
  border-radius: 4px;
  color: var(--text-2);
  font-weight: 560;
  cursor: pointer;
}
.opt:hover {
  color: var(--text);
}
.opt.on {
  background: var(--surface);
  color: var(--text);
  box-shadow: var(--shadow-1);
}
input {
  position: absolute;
  opacity: 0;
  inset: 0;
  margin: 0;
  cursor: pointer;
}
.opt:has(input:focus-visible) {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
}
</style>
