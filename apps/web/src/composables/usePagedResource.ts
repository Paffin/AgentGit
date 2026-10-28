import { onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import type { WatchSource } from 'vue'
import type { Page } from '@/api/types'
import { isApiError } from '@/api/errors'

/** A keyset list: cancel obsolete pages and never append data from a previous account or task. */
export function usePagedResource<T>(
  fetchPage: (cursor: string, signal: AbortSignal) => Promise<Page<T>>,
  scope: WatchSource[],
) {
  const data = shallowRef<T[]>([])
  const next = ref('')
  const loading = ref(false)
  const error = shallowRef<unknown>(null)
  let controller: AbortController | undefined
  let generation = 0
  let depth = 1
  let disposed = false

  async function load(append: boolean) {
    if (disposed || (append && (loading.value || !next.value))) return
    controller?.abort()
    const mine = ++generation
    controller = new AbortController()
    loading.value = true
    error.value = null
    try {
      let cursor = append ? next.value : ''
      const items: T[] = []
      let pages = 0
      do {
        const page = await fetchPage(cursor, controller.signal)
        if (mine !== generation) return
        items.push(...page.items)
        cursor = page.nextPageToken
        pages++
      } while (!append && cursor && pages < depth)
      data.value = append ? [...data.value, ...items] : items
      next.value = cursor
      depth = append ? depth + 1 : pages
    } catch (cause) {
      if (mine === generation && !(isApiError(cause) && cause.isCancelled)) error.value = cause
    } finally {
      if (mine === generation) loading.value = false
    }
  }
  const reload = () => load(false)
  const more = () => load(true)
  watch(
    scope,
    () => {
      data.value = []
      next.value = ''
      depth = 1
      void reload()
    },
    { immediate: true, flush: 'sync' },
  )
  onBeforeUnmount(() => {
    disposed = true
    generation++
    controller?.abort()
  })
  return { data, next, loading, error, reload, more }
}
