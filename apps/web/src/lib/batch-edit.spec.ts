// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { batchChanges, restoreBatch, type DraftFile } from './batch-edit'
import { decodeUtf8 } from './text'
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

describe('existing text file formats', () => {
  const bytesOf = (content: string) => Array.from(atob(content), (c) => c.charCodeAt(0))
  it.each(['\ufeffa\n', '\ufeffa\r\n', 'a\r\nb\n'])(
    'preserves all bytes on a rename: %j',
    (source) => {
      const bytes = new TextEncoder().encode(source)
      const decoded = decodeUtf8(bytes)
      const changes = batchChanges([
        file({ path: 'renamed.txt', original: decoded, text: decoded }),
      ])
      const put = changes.find((change) => change.op === 'put')!
      expect(bytesOf(put.content!)).toEqual(Array.from(bytes))
    },
  )
  it('retains BOM and CRLF after a textarea edit and rename', () => {
    const original = decodeUtf8(new TextEncoder().encode('\ufefffirst\r\nsecond\r\n'))
    const changes = batchChanges([
      file({ path: 'renamed.txt', original, text: 'changed\nsecond\n' }),
    ])
    const put = changes.find((change) => change.op === 'put')!
    expect(bytesOf(put.content!)).toEqual(
      Array.from(new TextEncoder().encode('\ufeffchanged\r\nsecond\r\n')),
    )
  })
  it('does not introduce a BOM or change LF for an ordinary file', () => {
    const changes = batchChanges([file({ original: 'first\n', text: 'changed\n' })])
    expect(bytesOf(changes[0]!.content!)).toEqual(Array.from(new TextEncoder().encode('changed\n')))
  })
})
