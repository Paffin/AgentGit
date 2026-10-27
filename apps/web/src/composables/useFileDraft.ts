import { ref } from 'vue'
import {
  readDraft,
  removeDraft,
  saveDraft,
  type DraftScope,
  type DraftStorage,
  type FileDraft,
} from '@/lib/drafts'

/** Synchronous tab storage closes the gap between an input event and a reload. */
export function useFileDraft(scope: DraftScope, storage?: DraftStorage) {
  let initial: FileDraft | null = null
  const error = ref('')
  const blocked = ref(false)
  const saved = ref(false)
  const restored = ref(false)
  try {
    initial = readDraft(storage ?? sessionStorage, scope)
    saved.value = initial !== null
    restored.value = initial !== null
  } catch {
    blocked.value = true
    error.value = 'The saved draft could not be read. It has been left untouched.'
  }

  function save(value: Omit<FileDraft, 'version' | 'savedAt'>) {
    if (blocked.value) return
    try {
      saveDraft(storage ?? sessionStorage, scope, { ...value, savedAt: Date.now() })
      saved.value = true
      error.value = ''
    } catch {
      saved.value = false
      error.value =
        'This tab could not save your draft. Download a copy before leaving or reloading.'
    }
  }

  function clear(): boolean {
    try {
      removeDraft(storage ?? sessionStorage, scope)
      saved.value = false
      restored.value = false
      blocked.value = false
      error.value = ''
      return true
    } catch {
      error.value = 'The saved draft could not be removed from this tab.'
      return false
    }
  }
  return { initial, error, blocked, saved, restored, save, clear }
}
