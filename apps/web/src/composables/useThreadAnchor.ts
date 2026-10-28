import { computed, ref, watch } from 'vue'
import type { LineQuote } from '@/lib/diffDiscussion'

function readAnchor(key: string): LineQuote | null {
  const raw = sessionStorage.getItem(key)
  if (!raw) return null
  if (raw.length > 65536) throw new Error('oversized anchor')
  const value = JSON.parse(raw)
  if (
    !['head', 'base', 'path', 'text'].every((k) => typeof value[k] === 'string') ||
    !['old', 'new'].includes(value.side) ||
    !Number.isSafeInteger(value.line) ||
    value.line < 1
  )
    throw new Error('invalid anchor')
  return value
}

/** Keep the selected line with its draft across reloads, scoped to the account and proposal. */
export function useThreadAnchor(scope: () => string) {
  const anchor = ref<LineQuote | null>(null)
  const error = ref('')
  const key = computed(() => `agentgit.thread-anchor.v1:${scope()}`)
  let restoring = false
  watch(
    key,
    () => {
      restoring = true
      anchor.value = null
      error.value = ''
      try {
        anchor.value = readAnchor(key.value)
      } catch {
        error.value = 'The saved line could not be restored. Select it again from Files changed.'
      } finally {
        restoring = false
      }
    },
    { immediate: true, flush: 'sync' },
  )
  watch(
    anchor,
    (value) => {
      if (restoring) return
      try {
        if (value) sessionStorage.setItem(key.value, JSON.stringify(value))
        else sessionStorage.removeItem(key.value)
        error.value = ''
      } catch {
        error.value = 'This tab could not save the selected line. Keep it open until you post.'
      }
    },
    { flush: 'sync' },
  )
  return { anchor, error }
}
