// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import type { Api } from '@/api/endpoints'
import type { TreeEntry } from '@/api/types'
import { ApiError } from '@/api/errors'
import { loadConflictVersion, conflictChange } from './conflictResolution'

const entry = (over: Partial<TreeEntry> = {}): TreeEntry => ({
  path: 'file',
  blob_id: 'original-blob',
  file_type: 'regular',
  executable: true,
  size: 3,
  ...over,
})
function fixture() {
  const raw = {
    tree: {
      resolve: vi.fn().mockResolvedValue({ kind: 'regular', entry: entry() }),
      file: vi.fn().mockResolvedValue({ bytes: new Uint8Array([0, 1, 255]), truncated: false }),
    },
  }
  return { raw, api: raw as unknown as Api }
}
const request = { token: 'original-session', quietUnauthenticated: true }
describe('conflict versions', () => {
  it('retains binary blob identity and executable mode without re-encoding it', async () => {
    const { raw, api } = fixture()
    const version = await loadConflictVersion(api, 'acme/app', 'pinned-head', 'file', request)
    expect(version.text).toBeNull()
    expect(conflictChange('file', 'mine', version, { entry: null, text: null }, '')).toEqual({
      path: 'file',
      op: 'put',
      blob_id: 'original-blob',
      file_type: 'regular',
      executable: true,
    })
    expect(raw.tree.resolve).toHaveBeenCalledWith('acme/app', 'pinned-head', 'file', request)
  })
  it('does not download oversized files and keeps a symbolic link as a symbolic link', async () => {
    const { raw, api } = fixture()
    raw.tree.resolve.mockResolvedValue({ kind: 'regular', entry: entry({ size: 2 * 1024 * 1024 }) })
    const large = await loadConflictVersion(api, 'acme/app', 'head', 'file', request)
    expect(large.text).toBeNull()
    expect(raw.tree.file).not.toHaveBeenCalled()
    const link = { entry: entry({ file_type: 'symlink', executable: false }), text: 'target' }
    expect(conflictChange('file', 'theirs', large, link, '')).toEqual({
      path: 'file',
      op: 'put',
      blob_id: 'original-blob',
      file_type: 'symlink',
      executable: false,
    })
  })
  it('recognizes only a definite missing path as deletion, never a failed read', async () => {
    const { raw, api } = fixture()
    raw.tree.resolve.mockResolvedValueOnce({ kind: 'missing', entry: null })
    const missing = await loadConflictVersion(api, 'acme/app', 'head', 'file', request)
    expect(conflictChange('file', 'mine', missing, missing, '')).toEqual({
      path: 'file',
      op: 'delete',
    })
    for (const status of [403, 404, 503]) {
      raw.tree.resolve.mockRejectedValueOnce(
        new ApiError({
          status,
          code: status === 404 ? 'not_found' : 'unavailable',
          message: 'Failed read',
        }),
      )
      await expect(loadConflictVersion(api, 'acme/app', 'head', 'file', request)).rejects.toThrow(
        'Failed read',
      )
    }
  })
  it('preserves BOM, CRLF and mode when text is explicitly edited', () => {
    const version = { entry: entry(), text: '\uFEFFold\r\n' }
    const change = conflictChange('file', 'edit', version, version, 'new\n', 'mine')
    expect(change.executable).toBe(true)
    expect(
      new TextDecoder('utf8', { ignoreBOM: true }).decode(
        Uint8Array.from(atob(change.content!), (x) => x.charCodeAt(0)),
      ),
    ).toBe('\uFEFFnew\r\n')
    expect(() => conflictChange('file', '', version, version, '')).toThrow(/Choose/)
  })
})

it('keeps malformed UTF-8 as an intact non-text version and propagates download failure', async () => {
  const { raw, api } = fixture()
  raw.tree.file.mockResolvedValueOnce({ bytes: new Uint8Array([0xff, 0x61]), truncated: false })
  const version = await loadConflictVersion(api, 'acme/app', 'head', 'file', request)
  expect(version.text).toBeNull()
  expect(version.entry?.blob_id).toBe('original-blob')
  raw.tree.file.mockRejectedValueOnce(
    new ApiError({ status: 503, code: 'unavailable', message: 'Download failed' }),
  )
  await expect(loadConflictVersion(api, 'acme/app', 'head', 'file', request)).rejects.toThrow(
    'Download failed',
  )
})

it('refuses incomplete or unknown file metadata before fetching bytes', async () => {
  const { raw, api } = fixture()
  for (const value of [null, entry({ blob_id: '' }), entry({ file_type: 'future-type' })]) {
    raw.tree.resolve.mockResolvedValue({ kind: 'regular', entry: value })
    await expect(loadConflictVersion(api, 'acme/app', 'head', 'file', request)).rejects.toThrow(
      /Cannot inspect/,
    )
  }
  expect(raw.tree.file).not.toHaveBeenCalled()
})
it('previews the exact size boundary and edits only the selected regular text source', async () => {
  const { raw, api } = fixture()
  raw.tree.resolve.mockResolvedValue({ kind: 'regular', entry: entry({ size: 512 * 1024 }) })
  raw.tree.file.mockResolvedValue({ bytes: new TextEncoder().encode('branch'), truncated: false })
  expect((await loadConflictVersion(api, 'acme/app', 'head', 'file', request)).text).toBe('branch')
  const mine = { entry: entry({ executable: true }), text: 'mine' }
  const theirs = { entry: entry({ executable: false }), text: 'theirs' }
  expect(conflictChange('file', 'edit', mine, theirs, 'new', 'theirs').executable).toBe(false)
  for (const invalid of [
    { entry: null, text: null },
    { entry: entry(), text: null },
    { entry: entry({ file_type: 'symlink' }), text: 'target' },
  ])
    expect(() => conflictChange('file', 'edit', invalid, theirs, 'new')).toThrow(/cannot be edited/)
})
