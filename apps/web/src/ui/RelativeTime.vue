<script setup lang="ts">
import { computed } from 'vue'
import { useServerNow } from '@/composables/useNow'
import { formatIso, formatRelative } from '@/lib/format'

const props = defineProps<{ ms: number }>()
const now = useServerNow()
const text = computed(() => formatRelative(props.ms, now.value))
const iso = computed(() => formatIso(props.ms))
</script>

<template>
  <time class="rel" :datetime="iso || undefined" :title="iso || undefined">{{ text }}</time>
</template>

<style scoped>
.rel {
  white-space: nowrap;
}
</style>
