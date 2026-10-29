// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { decodeUtf8 } from './text'
import { batchChanges, restoreBatch, type DraftFile } from './batch-edit'
const file = (over: Partial<DraftFile> = {}): DraftFile => ({
  id: '1',
  originalPath: 'a.txt',
  path: 'a.txt',
  original: 'old',
  text: 'new',
  executable: false,
  fileType: 'regular',
  removed: false,
  ...over,
})
describe('multi-file edits', () => {
  it.each([false, true])('preserves a BOM and CRLF when renaming (edited: %s)', (edited) => {
    const original = decodeUtf8(new TextEncoder().encode('\uFEFFfirst\r\nsecond\r\n'))
    const text = edited ? original.replace('second', 'changed') : original
    const changes = batchChanges([file({ path: 'renamed.txt', original, text })])
    const put = changes.find((change) => change.op === 'put')!
    const bytes = Uint8Array.from(atob(put.content!), (c) => c.charCodeAt(0))
    expect(Array.from(bytes)).toEqual(Array.from(new TextEncoder().encode(text)))
    expect(Array.from(bytes.slice(0, 3))).toEqual([0xef, 0xbb, 0xbf])
    expect(text).toContain('\r\n')
  })
  it('applies renames, edits and deletions as a single batch without losing modes', () => {
    const changes = batchChanges([
      file({ path: 'moved.txt', executable: true }),
      file({ id: '2', originalPath: 'gone.txt', path: 'gone.txt', removed: true }),
      file({ id: '3', originalPath: null, path: 'new.txt', original: '', text: 'added' }),
    ])
    expect(changes.map((c) => [c.op, c.path])).toEqual([
      ['delete', 'a.txt'],
      ['delete', 'gone.txt'],
      ['put', 'moved.txt'],
      ['put', 'new.txt'],
    ])
    expect(changes[2]).toMatchObject({ executable: true, content: 'bmV3' })
  })
  it('refuses duplicate destinations and unsafe paths before sending anything', () => {
    expect(() => batchChanges([file(), file({ id: '2' })])).toThrow(/same path/)
    expect(() => batchChanges([file({ path: '../outside' })])).toThrow(/path/)
  })
  it('restores a scoped draft with its original base and rejects corrupt data', () => {
    const draft = {
      version: 1,
      base: 'base',
      files: [file()],
      title: 'Both files',
      description: '',
    }
    expect(restoreBatch(JSON.stringify(draft))).toEqual(draft)
    expect(() => restoreBatch(JSON.stringify({ ...draft, files: [{ path: 'bad' }] }))).toThrow(
      /draft/,
    )
  })
})
