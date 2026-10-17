<script setup lang="ts">
import Button from './Button.vue'

defineProps<{ loading?: boolean; hasMore: boolean; shown: number; noun: string }>()
defineEmits<{ more: [] }>()
</script>

<template>
  <div class="more">
    <span class="count faint">{{ shown }} {{ noun }}{{ hasMore ? ' so far' : '' }}</span>
    <Button v-if="hasMore" size="sm" :loading="loading" @click="$emit('more')">Load more</Button>
  </div>
</template>

<style scoped>
.more {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  padding: var(--s-3) var(--s-4);
  font-size: var(--fs-sm);
  border-top: 1px solid var(--border);
}
</style>
