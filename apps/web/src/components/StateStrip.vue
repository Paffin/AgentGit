<script setup lang="ts">
import type { FourStates } from '@/lib/fourstate'
import { statesInOrder } from '@/lib/fourstate'
import StatePill from '@/ui/StatePill.vue'

defineProps<{ states: FourStates; compact?: boolean }>()
</script>

<template>
  <ul class="strip" role="list" aria-label="Merge readiness, four independent states">
    <li v-for="s in statesInOrder(states)" :key="s.key">
      <StatePill :state="s" :compact="compact" />
    </li>
  </ul>
</template>

<style scoped>
.strip {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
</style>
