import { onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'
import { client } from '@/api/instance'
import { openEventFeed, type EventFeed, type FeedStatus } from '@/api/sse'
import { useAuth } from '@/stores/auth'

/** One visible, selected repository gets a live feed; the aggregate view keeps bounded polling. */
export function useAttentionFeed(repository: Ref<string>, refresh: () => void) {
  const auth = useAuth()
  const status = ref<FeedStatus>('idle')
  const visible = ref(document.visibilityState === 'visible')
  let feed: EventFeed | undefined
  let generation = 0
  let mounted = false

  function stop() {
    generation++
    feed?.close()
    feed = undefined
    status.value = 'idle'
  }
  function reconcile() {
    stop()
    if (!mounted || !visible.value || !repository.value || !auth.isAuthenticated) return
    const mine = generation
    status.value = 'connecting'
    feed = openEventFeed({
      client,
      repo: repository.value,
      session: { token: auth.token, quietUnauthenticated: true },
      onEvent: () => {
        if (mine === generation) refresh()
      },
      onStatus: (next) => {
        if (mine !== generation) return
        status.value = next
        // Refresh after initial connection and reconnection to close the snapshot/feed gap.
        if (next === 'live') refresh()
      },
    })
  }
  function visibilityChanged() {
    visible.value = document.visibilityState === 'visible'
  }
  watch([repository, () => auth.token, () => auth.isAuthenticated, visible], reconcile, {
    flush: 'sync',
  })
  onMounted(() => {
    mounted = true
    document.addEventListener('visibilitychange', visibilityChanged)
    reconcile()
  })
  onBeforeUnmount(() => {
    mounted = false
    stop()
    document.removeEventListener('visibilitychange', visibilityChanged)
  })
  return status
}
