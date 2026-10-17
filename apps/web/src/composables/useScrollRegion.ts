import { computed, nextTick, onBeforeUnmount, onMounted, onUpdated, ref } from 'vue'
import type { Ref } from 'vue'

/**
 * Makes a box that scrolls sideways reachable without a mouse. A scroll container the keyboard cannot reach is an
 * accessibility defect (axe: scrollable-region-focusable), but a tab stop on every box that merely *could*
 * scroll is noise, so this measures: while the content really is wider than the box, the box is a labelled,
 * focusable region (the arrow keys scroll it); otherwise it is an ordinary element.
 *
 *   const { setEl, attrs } = useScrollRegion(() => `Changes to ${file.path}`)
 *   <div :ref="setEl" v-bind="attrs">
 */
export function useScrollRegion(label: () => string): {
  /** A function ref for the box: `<div :ref="setEl">`. */
  setEl: (el: unknown) => void
  scrolls: Ref<boolean>
  attrs: Readonly<Ref<{ tabindex?: 0; role?: 'region'; 'aria-label'?: string }>>
  measure: () => void
} {
  const el = ref<HTMLElement | null>(null)
  const scrolls = ref(false)
  const setEl = (node: unknown) => {
    el.value = node instanceof HTMLElement ? node : null
  }
  let observer: ResizeObserver | undefined

  function measure() {
    const box = el.value
    scrolls.value = !!box && box.scrollWidth > box.clientWidth + 1
  }
  onMounted(() => {
    measure()
    if (typeof ResizeObserver !== 'undefined' && el.value) {
      observer = new ResizeObserver(measure)
      observer.observe(el.value)
    }
  })
  // The content can outgrow the box without the box resizing: measure again after every render.
  onUpdated(() => void nextTick(measure))
  onBeforeUnmount(() => observer?.disconnect())

  const attrs = computed(() =>
    scrolls.value
      ? {
          tabindex: 0 as const,
          role: 'region' as const,
          'aria-label': `${label()} (scrolls sideways)`,
        }
      : {},
  )
  return { setEl, scrolls, attrs, measure }
}
