<script setup lang="ts">
withDefaults(defineProps<{ size?: number; label?: string }>(), { size: 16, label: undefined })
</script>

<template>
  <span class="spinner" :style="{ width: `${size}px`, height: `${size}px` }" role="status">
    <span v-if="label" class="sr-only">{{ label }}</span>
  </span>
</template>

<style scoped>
.spinner {
  display: inline-block;
  flex: none;
  border-radius: 50%;
  border: 2px solid currentColor;
  border-right-color: transparent;
  opacity: 0.7;
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
@media (prefers-reduced-motion: reduce) {
  .spinner {
    animation: none;
    border-right-color: currentColor;
    border-style: dotted;
  }
}
</style>
