<script setup lang="ts">
import { useId } from 'vue'

withDefaults(
  defineProps<{ modelValue: boolean; label: string; hint?: string; disabled?: boolean }>(),
  { hint: undefined, disabled: false },
)

const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>()
const id = useId()
</script>

<template>
  <div class="check">
    <input
      :id="id"
      type="checkbox"
      :checked="modelValue"
      :disabled="disabled"
      :aria-describedby="hint ? `${id}-hint` : undefined"
      @change="emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
    />
    <div>
      <label :for="id">{{ label }}</label>
      <p v-if="hint" :id="`${id}-hint`" class="hint">{{ hint }}</p>
    </div>
  </div>
</template>

<style scoped>
.check {
  display: flex;
  gap: var(--s-2);
  align-items: flex-start;
}
input {
  width: 16px;
  height: 16px;
  margin: 3px 0 0;
  accent-color: var(--accent);
  flex: none;
}
label {
  font-weight: 560;
}
.hint {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
</style>
