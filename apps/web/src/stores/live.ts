import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import { api, client } from '@/api/instance'
import { isApiError } from '@/api/errors'
import type { ApiError } from '@/api/errors'
import { openEventFeed } from '@/api/sse'
import type { EventFeed, FeedStatus, FeedStatusInfo } from '@/api/sse'
import type { ServerEvent } from '@/api/types'
import { describeEvent } from '@/lib/events'
import type { EventView } from '@/lib/events'

const KEEP = 300
const BASELINE = 120

export type LiveMessage = { kind: 'event'; event: ServerEvent } | { kind: 'resync' }

/**
 * The repository's live event stream. Opened once per repository page and shared: the board, the
 * proposal view and the activity timeline all listen to the same connection.
 */
export const useLive = defineStore('live', () => {
  const repo = ref<string | null>(null)
  const status = ref<FeedStatus>('idle')
  const retryInMs = ref(0)
  const error = ref<ApiError | null>(null)
  const events = shallowRef<EventView[]>([])
  const latestSeq = ref(0)
  const subscribers = new Set<(m: LiveMessage) => void>()
  let feed: EventFeed | null = null
  let serial = 0

  function notify(m: LiveMessage) {
    for (const fn of subscribers) {
      try {
        fn(m)
      } catch {
        // A faulty listener must not stop the others or the stream.
      }
    }
  }

  function subscribe(fn: (m: LiveMessage) => void): () => void {
    subscribers.add(fn)
    return () => subscribers.delete(fn)
  }

  function onEvent(ev: ServerEvent) {
    latestSeq.value = Math.max(latestSeq.value, ev.seq)
    events.value = [{ ...describeEvent(ev), liveAt: Date.now() }, ...events.value].slice(0, KEEP)
    notify({ kind: 'event', event: ev })
  }

  function onStatus(s: FeedStatus, info: FeedStatusInfo) {
    const was = status.value
    status.value = s
    retryInMs.value = info.retryInMs ?? 0
    error.value = info.error ?? null
    // Coming back after a gap: anything could have changed that we missed events for. Resync.
    if (s === 'live' && was === 'reconnecting') notify({ kind: 'resync' })
  }

  async function start(r: string): Promise<void> {
    if (repo.value === r && feed) return
    stop()
    repo.value = r
    const mine = ++serial
    status.value = 'connecting'
    let afterSeq: number | undefined
    try {
      const head = await api.events.replay(r, { after_seq: 0, limit: 1 })
      if (mine !== serial) return
      afterSeq = head.latestSeq
      latestSeq.value = head.latestSeq
      if (head.latestSeq > 0) {
        const tail = await api.events.replay(r, {
          after_seq: Math.max(0, head.latestSeq - BASELINE),
          limit: BASELINE + 20,
        })
        if (mine !== serial) return
        events.value = tail.events.map(describeEvent).reverse().slice(0, KEEP)
      }
    } catch (e) {
      if (mine !== serial) return
      if (isApiError(e) && (e.isForbidden || e.isNotFound || e.isUnauthenticated)) {
        error.value = e
        status.value = e.isUnauthenticated ? 'unauthenticated' : 'denied'
        return
      }
      // Could not baseline (server unreachable?). Follow from "now"; the feed keeps retrying on its own.
    }
    if (mine !== serial) return
    // Callbacks of a feed that was replaced or stopped must not touch the new state.
    feed = openEventFeed({
      client,
      repo: r,
      afterSeq,
      onEvent: (ev) => {
        if (mine === serial) onEvent(ev)
      },
      onStatus: (s, info) => {
        if (mine === serial) onStatus(s, info)
      },
    })
  }

  function stop() {
    serial++
    feed?.close()
    feed = null
    status.value = 'idle'
    retryInMs.value = 0
    error.value = null
    events.value = []
    latestSeq.value = 0
    repo.value = null
  }

  return { repo, status, retryInMs, error, events, latestSeq, start, stop, subscribe }
})
