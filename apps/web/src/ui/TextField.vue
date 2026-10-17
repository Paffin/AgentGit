<script setup lang="ts">
import { computed, useId } from 'vue'

const props = withDefaults(
  defineProps<{
    modelValue: string
    label: string
    type?: 'text' | 'password' | 'search' | 'number' | 'email'
    placeholder?: string
    hint?: string
    error?: string
    mono?: boolean
    disabled?: boolean
    readonly?: boolean
    required?: boolean
    autocomplete?: string
    /** Visually hide the label (it stays available to assistive tech). */
    hideLabel?: boolean
    name?: string
    min?: number
    max?: number
    /** The dialog this field is in focuses it first. */
    autofocus?: boolean
    /** id of a <datalist> offering suggestions. */
    list?: string
  }>(),
  {
    type: 'text',
    placeholder: undefined,
    hint: undefined,
    error: undefined,
    mono: false,
    disabled: false,
    readonly: false,
    required: false,
    autocomplete: 'off',
    hideLabel: false,
    name: undefined,
    min: undefined,
    max: undefined,
    autofocus: false,
    list: undefined,
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
    <div class="control" :class="{ invalid: !!error }">
      <slot name="prefix" />
      <input
        :id="id"
        :value="modelValue"
        :type="type"
        :name="name"
        :placeholder="placeholder"
        :disabled="disabled"
        :readonly="readonly"
        :required="required"
        :autocomplete="autocomplete"
        :min="min"
        :max="max"
        :list="list"
        :aria-invalid="error ? 'true' : undefined"
        :aria-describedby="describedBy"
        :class="{ mono }"
        :data-autofocus="autofocus ? '' : undefined"
        spellcheck="false"
        @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
      />
      <slot name="suffix" />
    </div>
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
.control {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  padding: 0 var(--s-2);
  min-height: max(34px, var(--touch-min));
}
.control:focus-within {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
  border-color: var(--focus);
}
.control.invalid {
  border-color: var(--bad);
}
input {
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  padding: 6px 2px;
  outline: none;
}
input::placeholder {
  color: var(--text-3);
}
input:disabled {
  opacity: 0.6;
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
