import { computed, onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'
import { client } from '@/api/instance'
import { openEventFeed, type EventFeed, type FeedStatus } from '@/api/sse'
import { useAuth } from '@/stores/auth'

/** One visible feed for the selected repository or bounded aggregate snapshot. */
export function useAttentionFeed(
  repository: Ref<string>,
  refresh: () => void,
  loadedRepositories: Ref<string[]> = ref([]),
) {
  const auth = useAuth()
  const status = ref<FeedStatus>('idle')
  const aggregateScope = computed(() => [...new Set(loadedRepositories.value.slice(0, 30))].sort())
  const scopeKey = computed(
    () =>
      repository.value ||
      (auth.capabilities?.features.includes('events.multi_repo') && aggregateScope.value.length
        ? JSON.stringify(aggregateScope.value)
        : ''),
  )
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
    if (!mounted || !visible.value || !scopeKey.value || !auth.isAuthenticated) return
    const mine = generation
    status.value = 'connecting'
    feed = openEventFeed({
      client,
      ...(repository.value ? { repo: repository.value } : { repos: aggregateScope.value }),
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
  watch([scopeKey, () => auth.token, () => auth.isAuthenticated, visible], reconcile, {
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
