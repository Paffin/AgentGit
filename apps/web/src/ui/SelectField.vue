<script setup lang="ts">
import { useId } from 'vue'

withDefaults(
  defineProps<{
    modelValue: string
    label: string
    options: readonly { value: string; label: string; disabled?: boolean }[]
    hint?: string
    hideLabel?: boolean
    disabled?: boolean
  }>(),
  { hint: undefined, hideLabel: false, disabled: false },
)

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const id = useId()
</script>

<template>
  <div class="field">
    <label :for="id" :class="{ 'sr-only': hideLabel }">{{ label }}</label>
    <select
      :id="id"
      :value="modelValue"
      :disabled="disabled"
      :aria-describedby="hint ? `${id}-hint` : undefined"
      @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value)"
    >
      <option v-for="o in options" :key="o.value" :value="o.value" :disabled="o.disabled">
        {{ o.label }}
      </option>
    </select>
    <p v-if="hint" :id="`${id}-hint`" class="hint">{{ hint }}</p>
  </div>
</template>

<style scoped>
.field {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  min-width: 0;
}
label {
  font-weight: 600;
  font-size: var(--fs-sm);
}
select {
  min-height: max(40px, var(--touch-min));
  padding: 0 var(--s-2);
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
}
select:focus {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
}
.hint {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
</style>
