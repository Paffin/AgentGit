<script setup lang="ts">
import { computed } from 'vue'
import type { GraphRow } from '@/lib/graph'

const props = defineProps<{ row: GraphRow; lanes: number; height?: number }>()

const LANE_W = 14
const PAD = 9
const H = computed(() => props.height ?? 44)
const width = computed(() => PAD * 2 + Math.max(0, props.lanes - 1) * LANE_W)
const x = (lane: number) => PAD + lane * LANE_W
const mid = computed(() => H.value / 2)
const COLORS = [
  'var(--accent)',
  'var(--agent)',
  'var(--ok)',
  'var(--warn)',
  'var(--info)',
  'var(--bad)',
  'var(--service)',
]
const color = (lane: number) => COLORS[lane % COLORS.length]

interface Seg {
  d: string
  color: string
}
const segments = computed<Seg[]>(() => {
  const r = props.row
  const out: Seg[] = []
  const h = H.value
  const m = mid.value
  // Lines that pass straight through this row.
  r.above.forEach((id, l) => {
    if (id !== null && id !== r.id && r.below[l] === id)
      out.push({ d: `M${x(l)} 0V${h}`, color: color(l) as string })
  })
  // Lines arriving at this commit from above.
  for (const l of r.incoming) {
    out.push({
      d:
        l === r.lane
          ? `M${x(l)} 0V${m}`
          : `M${x(l)} 0C${x(l)} ${m * 0.6} ${x(r.lane)} ${m * 0.4} ${x(r.lane)} ${m}`,
      color: color(l) as string,
    })
  }
  // Lines leaving towards each parent.
  r.parentLanes.forEach((pl) => {
    out.push({
      d:
        pl === r.lane
          ? `M${x(r.lane)} ${m}V${h}`
          : `M${x(r.lane)} ${m}C${x(r.lane)} ${m + (h - m) * 0.6} ${x(pl)} ${m + (h - m) * 0.4} ${x(pl)} ${h}`,
      color: color(pl) as string,
    })
  })
  return out
})
</script>

<template>
  <svg class="g" :width="width" :height="H" aria-hidden="true" focusable="false">
    <path
      v-for="(s, i) in segments"
      :key="i"
      :d="s.d"
      :stroke="s.color"
      fill="none"
      stroke-width="2"
      stroke-linecap="round"
    />
    <circle
      :cx="x(row.lane)"
      :cy="mid"
      :r="row.isMerge ? 5 : 4"
      :fill="row.isMerge ? 'var(--surface)' : (color(row.lane) as string)"
      :stroke="color(row.lane) as string"
      stroke-width="2"
    />
  </svg>
</template>

<style scoped>
.g {
  flex: none;
  display: block;
}
</style>
