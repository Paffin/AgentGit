import { computed, ref, watch } from 'vue'
import { randomHex } from '@/api/client'
export interface IssueDraft {
  title: string
  body: string
  taskId: string
  version: number
  requestId: string
}
const fresh = (): IssueDraft => ({
  title: '',
  body: '',
  taskId: '',
  version: 0,
  requestId: randomHex(),
})
/** Account/repository scoped tab recovery; credentials are never serialized. */
export function useIssueDraft(scope: () => string) {
  const key = computed(() => (scope() ? `agentgit.issue-draft:${scope()}` : ''))
  const draft = ref<IssueDraft>(fresh())
  const saved = ref(false)
  const error = ref('')
  let restoring = false
  watch(
    key,
    () => {
      restoring = true
      draft.value = fresh()
      saved.value = false
      error.value = ''
      if (key.value)
        try {
          const raw = sessionStorage.getItem(key.value)
          if (raw) {
            const value = JSON.parse(raw)
            if (
              !value ||
              !['title', 'body', 'taskId', 'requestId'].every(
                (k) => typeof value[k] === 'string',
              ) ||
              !Number.isSafeInteger(value.version) ||
              value.version < 0
            )
              throw new Error('invalid draft')
            draft.value = value
            saved.value = true
          }
        } catch {
          error.value = 'This tab could not restore the draft. The stored copy was left untouched.'
        }
      restoring = false
    },
    { immediate: true, flush: 'sync' },
  )
  watch(
    () => [draft.value.title, draft.value.body, draft.value.taskId, draft.value.version],
    () => {
      if (!restoring) draft.value.requestId = randomHex()
    },
    { flush: 'sync' },
  )
  watch(
    draft,
    () => {
      if (!key.value || restoring || error.value) return
      try {
        sessionStorage.setItem(key.value, JSON.stringify(draft.value))
        saved.value = true
      } catch {
        error.value = 'This tab could not save the draft. Copy your text before leaving.'
      }
    },
    { deep: true, flush: 'sync' },
  )
  function clear() {
    if (key.value)
      try {
        sessionStorage.removeItem(key.value)
      } catch {
        error.value = 'The saved draft could not be removed.'
        return
      }
    restoring = true
    draft.value = fresh()
    saved.value = false
    restoring = false
  }
  return { draft, saved, error, clear }
}
