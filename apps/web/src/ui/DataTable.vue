<script setup lang="ts" generic="Row extends object">
import { computed, nextTick, onBeforeUnmount, onMounted, onUpdated, ref, watch } from 'vue'
import { useViewportWidth } from '@/composables/useViewport'
import Skeleton from './Skeleton.vue'
import type { Column } from './types'

const props = withDefaults(
  defineProps<{
    columns: Column[]
    rows: readonly Row[]
    rowKey: (row: Row, index: number) => string
    /** Accessible name of the table. */
    caption: string
    loading?: boolean
    /** How many placeholder rows to show while loading (keeps the height stable). */
    skeletonRows?: number
    dense?: boolean
    selectedKey?: string
    /** Message when there are no rows (after loading). */
    emptyText?: string
  }>(),
  {
    loading: false,
    skeletonRows: 4,
    dense: false,
    selectedKey: undefined,
    emptyText: 'Nothing to show.',
  },
)

const width = useViewportWidth()

// The table scrolls sideways when it is wider than its container (a long description on a narrow screen). A
// scroll container that keyboard users cannot reach is an accessibility defect, so while it scrolls it is a
// labelled, focusable region (the arrow keys scroll it).
const wrap = ref<HTMLElement | null>(null)
const scrolls = ref(false)
let observer: ResizeObserver | undefined
function measure() {
  const el = wrap.value
  scrolls.value = !!el && el.scrollWidth > el.clientWidth + 1
}
onMounted(() => {
  measure()
  if (typeof ResizeObserver !== 'undefined' && wrap.value) {
    observer = new ResizeObserver(measure)
    observer.observe(wrap.value)
    const table = wrap.value.querySelector('table')
    if (table) observer.observe(table)
  }
})
// The content can outgrow the box without the box (or the table) resizing: re-measure after every render.
onUpdated(measure)
onBeforeUnmount(() => observer?.disconnect())
watch(
  () => [props.rows, props.loading, width.value],
  () => void nextTick(measure),
)
const visible = computed(() =>
  props.columns.filter((c) => !c.hideBelow || width.value >= c.hideBelow),
)
const template = computed(() => visible.value.map((c) => c.width ?? 'minmax(0, 1fr)').join(' '))

function value(row: Row, key: string): unknown {
  return (row as Record<string, unknown>)[key]
}
</script>

<template>
  <div
    ref="wrap"
    class="wrap"
    :tabindex="scrolls ? 0 : undefined"
    :role="scrolls ? 'region' : undefined"
    :aria-label="scrolls ? `${caption} (scrolls sideways)` : undefined"
  >
    <table class="table" :class="{ dense }" :aria-busy="loading ? 'true' : undefined">
      <caption class="sr-only">
        {{
          caption
        }}
      </caption>
      <thead>
        <tr :style="{ gridTemplateColumns: template }">
          <th
            v-for="c in visible"
            :key="c.key"
            scope="col"
            :class="[`a-${c.align ?? 'start'}`, { 'sr-only': c.srOnlyHeader }]"
          >
            {{ c.label }}
          </th>
        </tr>
      </thead>
      <tbody v-if="loading && rows.length === 0">
        <tr
          v-for="n in skeletonRows"
          :key="n"
          :style="{ gridTemplateColumns: template }"
          aria-hidden="true"
        >
          <td v-for="c in visible" :key="c.key">
            <Skeleton :width="n % 2 ? '70%' : '55%'" :height="12" />
          </td>
        </tr>
      </tbody>
      <tbody v-else-if="rows.length > 0">
        <tr
          v-for="(row, i) in rows"
          :key="rowKey(row, i)"
          :style="{ gridTemplateColumns: template }"
          :class="{
            selected: selectedKey !== undefined && rowKey(row, i) === selectedKey,
          }"
        >
          <td
            v-for="c in visible"
            :key="c.key"
            :class="[`a-${c.align ?? 'start'}`, { mono: c.mono }]"
          >
            <slot :name="`cell-${c.key}`" :row="row" :value="value(row, c.key)" :index="i">
              {{ value(row, c.key) ?? '' }}
            </slot>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-if="!loading && rows.length === 0" class="empty">
      <slot name="empty">{{ emptyText }}</slot>
    </div>
  </div>
</template>

<style scoped>
.wrap {
  /* Only sideways: with `overflow-x` alone the other axis computes to `auto` too, and a sub-pixel rounding
     difference in the height would make a scroll container of it that no keyboard can reach. */
  overflow-x: auto;
  overflow-y: hidden;
}
.table {
  width: 100%;
  border-collapse: collapse;
  display: block;
  min-width: min-content;
}
thead,
tbody {
  display: block;
}
tr {
  display: grid;
  align-items: center;
  column-gap: var(--s-3);
  padding: 0 var(--s-4);
  border-bottom: 1px solid var(--border);
}
tbody tr:last-child {
  border-bottom: none;
}
thead tr {
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--surface-2);
  border-bottom: 1px solid var(--border);
}
th {
  padding: var(--s-2) 0;
  text-align: start;
  font-size: var(--fs-xs);
  font-weight: 620;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  color: var(--text-3);
}
td {
  min-width: 0;
  padding: var(--s-3) 0;
  overflow-wrap: anywhere;
}
.dense td {
  padding: var(--s-2) 0;
}
tbody tr:hover {
  background: var(--surface-2);
}
tbody tr.selected {
  background: var(--accent-soft);
}
.a-end {
  text-align: end;
  justify-self: end;
}
.a-center {
  text-align: center;
  justify-self: center;
}
.mono {
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
}
.empty {
  padding: var(--s-6) var(--s-4);
  text-align: center;
  color: var(--text-2);
}
</style>
