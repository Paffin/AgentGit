<script setup lang="ts">
import { computed, useId } from 'vue'

const props = withDefaults(
  defineProps<{
    modelValue: string
    label: string
    rows?: number
    placeholder?: string
    hint?: string
    error?: string
    mono?: boolean
    disabled?: boolean
    hideLabel?: boolean
    required?: boolean
    maxlength?: number
  }>(),
  {
    rows: 4,
    placeholder: undefined,
    hint: undefined,
    error: undefined,
    mono: false,
    disabled: false,
    hideLabel: false,
    required: false,
    maxlength: undefined,
  },
)

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const id = useId()
const describedBy = computed(
  () =>
    [props.hint ? `${id}-hint` : '', props.error ? `${id}-err` : ''].filter(Boolean).join(' ') ||
    undefined,
)
</script>

<template>
  <div class="field">
    <label :for="id" :class="{ 'sr-only': hideLabel }">
      {{ label }}<span v-if="required" class="req" aria-hidden="true"> *</span>
    </label>
    <textarea
      :id="id"
      :value="modelValue"
      :rows="rows"
      :placeholder="placeholder"
      :disabled="disabled"
      :required="required"
      :maxlength="maxlength"
      :class="{ mono, invalid: !!error }"
      :aria-invalid="error ? 'true' : undefined"
      :aria-describedby="describedBy"
      spellcheck="false"
      @input="emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)"
    />
    <p v-if="hint" :id="`${id}-hint`" class="hint">{{ hint }}</p>
    <p v-if="error" :id="`${id}-err`" class="error" role="alert">{{ error }}</p>
  </div>
</template>

<style scoped>
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}
label {
  font-weight: 580;
  font-size: var(--fs-sm);
}
.req {
  color: var(--bad);
}
textarea {
  width: 100%;
  resize: vertical;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  padding: var(--s-2);
  line-height: var(--lh);
  min-height: 72px;
}
textarea:focus {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
  border-color: var(--focus);
}
textarea.invalid {
  border-color: var(--bad);
}
textarea::placeholder {
  color: var(--text-3);
}
.mono {
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
}
.hint {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.error {
  color: var(--bad);
  font-size: var(--fs-sm);
}
</style>
