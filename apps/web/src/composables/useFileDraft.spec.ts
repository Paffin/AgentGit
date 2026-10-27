import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useFileDraft } from './useFileDraft'
import { draftKey } from '@/lib/drafts'
const scope = {
  identity: 'user:writer',
  repo: 'team/repo',
  stream: 'main',
  path: 'file',
  isNew: false,
}
const value = {
  text: 'edited',
  original: 'before',
  path: 'file',
  base: 'immutable-base',
  executable: false,
  fileType: 'regular' as const,
  title: 'Edit',
  description: '',
}
beforeEach(() => sessionStorage.clear())
describe('draft save status', () => {
  it('recovers the saved contents, then removes only the current draft', () => {
    const first = useFileDraft(scope)
    expect(first.saved.value).toBe(false)
    expect(first.restored.value).toBe(false)
    first.save(value)
    expect(first.saved.value).toBe(true)
    const next = useFileDraft(scope)
    expect(next.initial).toMatchObject(value)
    expect(next.restored.value).toBe(true)
    expect(next.clear()).toBe(true)
    expect(next.saved.value).toBe(false)
    expect(next.restored.value).toBe(false)
    expect(useFileDraft(scope).initial).toBeNull()
  })
  it('leaves an unreadable draft alone until the user explicitly replaces it', () => {
    sessionStorage.setItem(draftKey(scope), 'broken')
    const state = useFileDraft(scope)
    expect(state.restored.value).toBe(false)
    state.save(value)
    expect(state.saved.value).toBe(false)
    expect(sessionStorage.getItem(draftKey(scope))).toBe('broken')
    state.clear()
    state.save(value)
    expect(state.saved.value).toBe(true)
  })
  it('reports a storage failure without claiming that the current text was saved', () => {
    const state = useFileDraft(scope, {
      getItem: () => null,
      removeItem: () => {},
      setItem: () => {
        throw new Error('quota')
      },
    })
    state.save(value)
    expect(state.saved.value).toBe(false)
    expect(state.error.value).toContain('Download a copy')
  })
  it('keeps the editor usable when the browser denies access to storage itself', () => {
    const getter = vi.spyOn(window, 'sessionStorage', 'get').mockImplementation(() => {
      throw new DOMException('Storage denied', 'SecurityError')
    })
    try {
      const state = useFileDraft(scope)
      expect(state.initial).toBeNull()
      expect(state.saved.value).toBe(false)
      expect(state.error.value).toContain('could not be read')
      expect(() => state.save(value)).not.toThrow()
      expect(state.clear()).toBe(false)
    } finally {
      getter.mockRestore()
    }
  })
  it('does not claim that a draft was removed when storage refuses the removal', () => {
    const state = useFileDraft(scope, {
      getItem: () => null,
      setItem: () => {},
      removeItem: () => {
        throw new Error('denied')
      },
    })
    expect(state.clear()).toBe(false)
    expect(state.error.value).toContain('could not be removed')
  })
})
