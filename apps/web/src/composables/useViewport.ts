import { onBeforeUnmount, onMounted, ref } from 'vue'

/** Current viewport width in CSS pixels, updated on resize. */
export function useViewportWidth() {
  const width = ref(typeof window === 'undefined' ? 1280 : window.innerWidth)
  const update = () => {
    width.value = window.innerWidth
  }
  onMounted(() => {
    update()
    window.addEventListener('resize', update, { passive: true })
  })
  onBeforeUnmount(() => window.removeEventListener('resize', update))
  return width
}
