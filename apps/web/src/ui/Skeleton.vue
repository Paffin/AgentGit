<script setup lang="ts">
withDefaults(
  defineProps<{
    width?: string | number
    height?: string | number
    radius?: string
    /** Several stacked lines (the last is shorter). */
    lines?: number
  }>(),
  { width: '100%', height: 14, radius: undefined, lines: 1 },
)

const px = (v: string | number) => (typeof v === 'number' ? `${v}px` : v)
</script>

<template>
  <span class="skel-group" aria-hidden="true">
    <span
      v-for="n in lines"
      :key="n"
      class="skel"
      :style="{
        width: lines > 1 && n === lines ? '60%' : px(width),
        height: px(height),
        borderRadius: radius,
      }"
    />
  </span>
</template>

<style scoped>
.skel-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
}
.skel {
  display: block;
  max-width: 100%;
  border-radius: 4px;
  background: linear-gradient(
    90deg,
    var(--surface-3) 0%,
    var(--surface-2) 50%,
    var(--surface-3) 100%
  );
  background-size: 200% 100%;
  animation: shimmer 1.6s linear infinite;
}
@keyframes shimmer {
  from {
    background-position: 100% 0;
  }
  to {
    background-position: -100% 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .skel {
    animation: none;
    background: var(--surface-3);
  }
}
</style>
