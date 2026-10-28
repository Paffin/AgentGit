import { computed, ref, watch } from 'vue'
import { randomHex } from '@/api/client'

interface SavedComment {
  body: string
  head: string
  requestId: string
}

function readSaved(key: string): SavedComment | null {
  const raw = key ? sessionStorage.getItem(key) : null
  if (!raw) return null
  if (raw.length > 131072) throw new Error('oversized draft')
  const saved = JSON.parse(raw)
  if (
    saved.version !== 1 ||
    ![saved.body, saved.head, saved.requestId].every((value) => typeof value === 'string')
  )
    throw new Error('invalid draft')
  return saved
}
function persist(key: string, draft: SavedComment | null) {
  if (!key) return
  if (!draft?.body) sessionStorage.removeItem(key)
  else sessionStorage.setItem(key, JSON.stringify({ version: 1, ...draft }))
}

/** A tab-local comment draft bound to an account, proposal and inspected head. */
export function useCommentDraft(scope: () => string, currentHead: () => string) {
  const storageKey = computed(() => (scope() ? `agentgit.proposal-comment.v1:${scope()}` : ''))
  const body = ref(''),
    head = ref(currentHead()),
    requestId = ref(randomHex()),
    error = ref('')
  let restoring = false,
    blocked = false
  watch(
    storageKey,
    () => {
      restoring = true
      body.value = ''
      head.value = currentHead()
      requestId.value = randomHex()
      error.value = ''
      blocked = false
      try {
        const saved = readSaved(storageKey.value)
        if (saved) {
          body.value = saved.body
          head.value = saved.head
          requestId.value = saved.requestId
        }
      } catch {
        blocked = true
        error.value =
          'The saved comment could not be restored and was left untouched. Copy new text before leaving.'
      } finally {
        restoring = false
      }
    },
    { immediate: true, flush: 'sync' },
  )
  watch(
    [body, head],
    () => {
      if (restoring) return
      requestId.value = randomHex()
      if (blocked) return
      try {
        persist(storageKey.value, {
          body: body.value,
          head: head.value,
          requestId: requestId.value,
        })
        error.value = ''
      } catch {
        error.value = 'This tab could not save the comment. Copy your text before leaving.'
      }
    },
    { flush: 'sync' },
  )
  watch(currentHead, (value) => {
    if (!body.value) head.value = value
  })
  function clear() {
    try {
      persist(storageKey.value, null)
    } catch {
      error.value = 'The saved comment could not be removed.'
      return
    }
    restoring = true
    body.value = ''
    head.value = currentHead()
    requestId.value = randomHex()
    blocked = false
    error.value = ''
    restoring = false
  }
  return { body, head, requestId, error, clear }
}
