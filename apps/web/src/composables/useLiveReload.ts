import { onBeforeUnmount } from 'vue'
import { coalesce } from '@/lib/pool'
import { useLive } from '@/stores/live'

/** Calls `reload` (coalesced) whenever an event whose topic starts with one of `prefixes` arrives, or the stream resynced. */
export function useLiveReload(
  prefixes: readonly string[],
  reload: () => void | Promise<void>,
  delay = 300,
) {
  const live = useLive()
  const run = coalesce(reload, delay)
  const off = live.subscribe((m) => {
    if (m.kind === 'resync') return run.trigger()
    const root = m.event.topic.split('.')[0] ?? ''
    if (prefixes.includes(root)) run.trigger()
  })
  onBeforeUnmount(() => {
    off()
    run.cancel()
  })
}
