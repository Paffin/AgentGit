import { computed, nextTick, onBeforeUnmount, onMounted, onUpdated, ref, watch } from 'vue'
import type { Ref } from 'vue'

/**
 * Makes a box that scrolls sideways reachable without a mouse. A scroll container the keyboard cannot reach is an
 * accessibility defect (axe: scrollable-region-focusable), but a tab stop on every box that merely *could*
 * scroll is noise, so this measures: while the content really is wider than the box, the box is a labelled,
 * focusable region (the arrow keys scroll it); otherwise it is an ordinary element.
 *
 * What can make the content outgrow the box while the box itself keeps its size, and so what is watched: the
 * content's own size (a ResizeObserver on the box *and* its children: a cell that fills in later, a web font
 * that finishes loading), any change of the DOM below the box that did not come from a re-render of the owner
 * (a child component updating itself: a relative time that ticks, a name that resolves; a MutationObserver
 * measures in the same microtask checkpoint, so there is no frame in which the box overflows unannounced) and
 * the fonts' `loadingdone`. Watching only the box's own size left a window in which an automated audit saw a
 * scroll container the keyboard could not reach (the flaky axe `scrollable-region-focusable` failure).
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
  function measure() {
    const box = el.value
    scrolls.value = !!box && box.scrollWidth > box.clientWidth + 1
  }

  // Each conditional/replaced DOM node owns its observers and cleanup.
  watch(
    el,
    (box, _previous, onCleanup) => {
      if (box) onCleanup(observeBox(box, measure))
      measure()
    },
    { flush: 'post' },
  )

  let stopFonts = () => {}
  onMounted(() => {
    stopFonts = observeFonts(measure)
  })
  onBeforeUnmount(() => stopFonts())
  // The content can outgrow the box without the box resizing: measure again after every render.
  onUpdated(() => void nextTick(measure))

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

/** Observe the current children, dropping references to children that were removed. */
function observeSizes(
  box: HTMLElement,
  resizes: ResizeObserver | undefined,
  watched: Set<Element>,
) {
  if (!resizes) return
  const now = new Set<Element>([box, ...Array.from(box.children)])
  for (const node of now) {
    if (!watched.has(node)) {
      watched.add(node)
      resizes.observe(node)
    }
  }
  for (const node of watched) {
    if (!now.has(node)) {
      watched.delete(node)
      resizes.unobserve(node)
    }
  }
}

function observeBox(box: HTMLElement, measure: () => void) {
  const watched = new Set<Element>()
  const resizes = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
  const contentChanged = () => {
    observeSizes(box, resizes, watched)
    measure()
  }
  const mutations =
    typeof MutationObserver === 'undefined' ? undefined : new MutationObserver(contentChanged)
  observeSizes(box, resizes, watched)
  mutations?.observe(box, { childList: true, subtree: true, characterData: true })
  return () => {
    resizes?.disconnect()
    mutations?.disconnect()
    watched.clear()
  }
}

function observeFonts(measure: () => void) {
  if (typeof document === 'undefined') return () => {}
  const fonts = document.fonts
  fonts?.addEventListener?.('loadingdone', measure)
  return () => fonts?.removeEventListener?.('loadingdone', measure)
}
