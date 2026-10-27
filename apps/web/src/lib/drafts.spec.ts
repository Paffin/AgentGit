import { beforeEach, describe, expect, it } from 'vitest'
import { draftKey, readDraft, saveDraft, removeDraft, type FileDraft } from './drafts'

const scope = {
  identity: 'user:alice',
  repo: 'team/project',
  stream: 'main',
  path: 'README.md',
  isNew: false,
}
const draft: FileDraft = {
  version: 1,
  text: 'Unsaved change\n',
  original: 'Original\n',
  path: 'README.md',
  base: 'old-base',
  executable: false,
  fileType: 'regular',
  title: 'My title',
  description: 'Why',
  savedAt: 123,
}
beforeEach(() => sessionStorage.clear())
describe('tab-scoped file drafts', () => {
  it('accepts the timestamp boundary and rejects malformed but parseable records without deleting them', () => {
    saveDraft(sessionStorage, scope, {
      ...draft,
      savedAt: 0,
      fileType: 'symlink',
      executable: true,
    })
    expect(readDraft(sessionStorage, scope)?.savedAt).toBe(0)
    for (const invalid of [
      null,
      true,
      7,
      'not a draft',
      [],
      { ...draft, version: 2 },
      { ...draft, text: 12 },
      { ...draft, executable: 'yes' },
      { ...draft, fileType: 'unknown' },
      { ...draft, savedAt: -1 },
      { ...draft, savedAt: null },
    ]) {
      const raw = JSON.stringify(invalid)
      sessionStorage.setItem(draftKey(scope), raw)
      expect(() => readDraft(sessionStorage, scope)).toThrow('could not be read')
      expect(sessionStorage.getItem(draftKey(scope))).toBe(raw)
    }
  })
  it('restores the exact text and immutable starting point across editor instances', () => {
    saveDraft(sessionStorage, scope, draft)
    expect(readDraft(sessionStorage, scope)).toEqual(draft)
    removeDraft(sessionStorage, scope)
    expect(readDraft(sessionStorage, scope)).toBeNull()
  })
  it('isolates accounts, repositories, branches, paths and new-file drafts', () => {
    saveDraft(sessionStorage, scope, draft)
    for (const other of [
      { identity: 'user:bob' },
      { repo: 'other/project' },
      { stream: 'release' },
      { path: 'other.md' },
      { isNew: true },
    ])
      expect(readDraft(sessionStorage, { ...scope, ...other })).toBeNull()
    expect(() => saveDraft(sessionStorage, { ...scope, identity: '' }, draft)).toThrow()
  })
  it('does not overwrite malformed data or silently pretend a failed save succeeded', () => {
    sessionStorage.setItem(draftKey(scope), '{broken')
    expect(() => readDraft(sessionStorage, scope)).toThrow('could not be read')
    expect(sessionStorage.getItem(draftKey(scope))).toBe('{broken')
    const unavailable = {
      setItem() {
        throw new Error('quota')
      },
      getItem() {
        return null
      },
      removeItem() {},
    }
    expect(() => saveDraft(unavailable, scope, draft)).toThrow()
  })
  it('writes only draft fields and never incidental credentials', () => {
    saveDraft(sessionStorage, scope, { ...draft, token: 'never-store-this' } as FileDraft)
    expect(sessionStorage.getItem(draftKey(scope))).not.toContain('never-store-this')
  })
})
