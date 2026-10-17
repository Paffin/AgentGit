import { onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import type { Ref, WatchSource } from 'vue'
import { isApiError } from '@/api/errors'

export interface Resource<T> {
  data: Ref<T | undefined>
  error: Ref<unknown>
  /** A request is in flight (also during a background reload). */
  loading: Ref<boolean>
  /** The first load finished (successfully or not). */
  settled: Ref<boolean>
  reload: () => Promise<void>
}

/**
 * Loads data and keeps it. Reloading keeps showing the previous data (no flicker, no layout shift),
 * only the first load shows a skeleton. A newer load always wins over an older one, and unmounting cancels.
 */
export function useResource<T>(
  load: (signal: AbortSignal) => Promise<T>,
  options: { watch?: WatchSource[]; immediate?: boolean } = {},
): Resource<T> {
  const data = shallowRef<T>()
  const error = shallowRef<unknown>(null)
  const loading = ref(false)
  const settled = ref(false)
  let controller: AbortController | null = null
  let serial = 0

  async function reload() {
    controller?.abort()
    const mine = ++serial
    const ctl = new AbortController()
    controller = ctl
    loading.value = true
    try {
      const value = await load(ctl.signal)
      if (mine !== serial) return
      data.value = value
      error.value = null
    } catch (e) {
      if (mine !== serial) return
      if (isApiError(e) && e.isCancelled) return
      error.value = e
    } finally {
      if (mine === serial) {
        loading.value = false
        settled.value = true
      }
    }
  }

  if (options.watch && options.watch.length > 0) {
    watch(options.watch, () => {
      data.value = undefined
      error.value = null
      settled.value = false
      void reload()
    })
  }
  if (options.immediate !== false) void reload()
  onBeforeUnmount(() => {
    serial++
    controller?.abort()
  })

  return { data, error, loading, settled, reload }
}
