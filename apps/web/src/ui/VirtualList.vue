<script setup lang="ts" generic="Item">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

/**
 * A fixed-row-height virtual list: only the rows in (or near) the viewport exist in the DOM, so a feed of
 * thousands of events costs the same as a feed of twenty.
 */
const props = withDefaults(
  defineProps<{
    items: readonly Item[]
    rowHeight: number
    itemKey: (item: Item, index: number) => string | number
    /** Height of the scrolling area (CSS value). */
    height?: string
    overscan?: number
    label: string
    /** Keep the view pinned to the top when new items are prepended and the user is at the top. */
    stickToTop?: boolean
  }>(),
  { height: '420px', overscan: 6, stickToTop: false },
)

const scroller = ref<HTMLElement | null>(null)
const scrollTop = ref(0)
const viewport = ref(0)

const total = computed(() => props.items.length * props.rowHeight)
const first = computed(() =>
  Math.max(0, Math.floor(scrollTop.value / props.rowHeight) - props.overscan),
)
const last = computed(() =>
  Math.min(
    props.items.length,
    Math.ceil((scrollTop.value + viewport.value) / props.rowHeight) + props.overscan,
  ),
)
const slice = computed(() => props.items.slice(first.value, last.value))

// Coalesce events and position rows outside scroll dispatch, preserving asynchronous panning.
let scrollFrame: number | undefined
function onScroll() {
  if (scrollFrame !== undefined) return
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = undefined
    scrollTop.value = scroller.value?.scrollTop ?? 0
  })
}

let ro: ResizeObserver | undefined
onMounted(() => {
  const el = scroller.value
  if (!el) return
  viewport.value = el.clientHeight || parseInt(props.height, 10) || 400
  if (typeof ResizeObserver !== 'undefined') {
    ro = new ResizeObserver(() => {
      viewport.value = el.clientHeight
    })
    ro.observe(el)
  }
})
onBeforeUnmount(() => {
  ro?.disconnect()
  if (scrollFrame !== undefined) cancelAnimationFrame(scrollFrame)
})

watch(
  () => props.items.length,
  async (n, old) => {
    if (!props.stickToTop || n <= old) return
    const el = scroller.value
    if (!el) return
    // New items were prepended: keep the reader's place unless they are already at the top.
    if (scrollTop.value > props.rowHeight) {
      await nextTick()
      el.scrollTop += (n - old) * props.rowHeight
    }
  },
)

defineExpose({
  scrollToIndex: (i: number) => scroller.value?.scrollTo({ top: i * props.rowHeight }),
})
</script>

<template>
  <div
    ref="scroller"
    class="vl"
    :style="{ height }"
    role="list"
    :aria-label="label"
    tabindex="0"
    @scroll.passive="onScroll"
  >
    <div class="space" :style="{ height: `${total}px` }">
      <div
        v-for="(item, i) in slice"
        :key="itemKey(item, first + i)"
        class="row"
        role="listitem"
        :aria-setsize="items.length"
        :aria-posinset="first + i + 1"
        :style="{ transform: `translateY(${(first + i) * rowHeight}px)`, height: `${rowHeight}px` }"
      >
        <slot :item="item" :index="first + i" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.vl {
  overflow-y: auto;
  position: relative;
  contain: strict;
}
.space {
  position: relative;
}
.row {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  overflow: hidden;
}
</style>
