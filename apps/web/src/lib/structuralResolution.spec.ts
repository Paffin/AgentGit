import { expect, it, vi } from 'vitest'
import {
  structuralRoots,
  loadStructuralSide,
  structuralChanges,
  scopePaths,
} from './structuralResolution'
const entry = (path: string, blob_id = path) => ({
  path,
  blob_id,
  file_type: 'regular',
  executable: true,
  size: 3,
})
it('groups every nested conflict under a minimal structural root', () => {
  expect(
    structuralRoots([
      { path: 'a/b', kind: 'content' },
      { path: 'a', kind: 'file_directory' },
      { path: 'a/nested', kind: 'file_directory' },
      { path: 'z', kind: 'content' },
    ] as never),
  ).toEqual(['a'])
})
it('requires complete bounded scope confined to the conflict root', () => {
  expect(scopePaths('a', { complete: true, paths: ['a', 'a/synthetic'] })).toEqual([
    'a',
    'a/synthetic',
  ])
  for (const scope of [
    null,
    { complete: false, paths: ['a'] },
    { complete: true, paths: ['a/b'] },
    { complete: true, paths: ['a', 'ab/file'] },
    { complete: true, paths: ['a', 'a/../escape'] },
  ])
    expect(() => scopePaths('a', scope)).toThrow()
  expect(() =>
    scopePaths('a', {
      complete: true,
      paths: ['a', ...Array.from({ length: 100 }, (_, i) => `a/${i}`)],
    }),
  ).toThrow()
})
it('preserves exact selected blobs and explicitly deletes synthetic and unselected paths', () => {
  expect(
    structuralChanges(['a', 'a/synthetic', 'a/other'], [entry('a/kept')], [entry('a/other')]),
  ).toEqual([
    { path: 'a', op: 'delete' },
    { path: 'a/kept', op: 'put', blob_id: 'a/kept', file_type: 'regular', executable: true },
    { path: 'a/other', op: 'delete' },
    { path: 'a/synthetic', op: 'delete' },
  ])
})
it('reads pinned paginated directory entries without downloading file bytes', async () => {
  const api = {
    tree: {
      resolve: vi.fn().mockResolvedValue({ kind: 'dir', entry: null }),
      list: vi
        .fn()
        .mockResolvedValueOnce({
          entries: [{ name: 'link', kind: 'symlink', blob_id: 'l', executable: false, size: 6 }],
          nextPageToken: 'next',
        })
        .mockResolvedValueOnce({
          entries: [{ name: 'run', kind: 'file', blob_id: 'r', executable: true, size: 3 }],
          nextPageToken: '',
        }),
    },
  }
  const signal = new AbortController().signal
  expect(
    await loadStructuralSide(api as never, 'repo', 'pinned', 'a', { signal, token: 'original' }),
  ).toEqual([
    { path: 'a/link', blob_id: 'l', file_type: 'symlink', executable: false, size: 6 },
    { path: 'a/run', blob_id: 'r', file_type: 'regular', executable: true, size: 3 },
  ])
  expect(api.tree.list).toHaveBeenLastCalledWith(
    'repo',
    { rev: 'pinned', path: 'a', page_size: 100, page_token: 'next' },
    { signal, token: 'original' },
  )
})
it('refuses unbounded, malformed, unreadable or cancelled source snapshots', async () => {
  const api = {
    tree: {
      resolve: vi.fn().mockResolvedValue({ kind: 'dir', entry: null }),
      list: vi.fn().mockResolvedValue({
        entries: Array.from({ length: 101 }, (_, i) => ({
          name: `f${i}`,
          kind: 'regular',
          blob_id: 'b',
          size: 0,
          executable: false,
        })),
        nextPageToken: '',
      }),
    },
  }
  await expect(loadStructuralSide(api as never, 'r', 'h', 'a', {})).rejects.toThrow()
  api.tree.list.mockResolvedValue({
    entries: [{ name: '../escape', kind: 'regular', blob_id: 'b' }],
    nextPageToken: '',
  })
  await expect(loadStructuralSide(api as never, 'r', 'h', 'a', {})).rejects.toThrow()
  api.tree.list.mockRejectedValue(new Error('Forbidden'))
  await expect(loadStructuralSide(api as never, 'r', 'h', 'a', {})).rejects.toThrow('Forbidden')
  const abort = new AbortController()
  abort.abort()
  await expect(
    loadStructuralSide(api as never, 'r', 'h', 'a', { signal: abort.signal }),
  ).rejects.toThrow()
})

it('shares request budget across roots and rejects excessive conflict counts', async () => {
  const api = { tree: { resolve: vi.fn().mockResolvedValue({ kind: 'missing', entry: null }) } }
  const budget = { remaining: 1 }
  await loadStructuralSide(api as never, 'r', 'h', 'a', {}, budget)
  await expect(loadStructuralSide(api as never, 'r', 'h', 'b', {}, budget)).rejects.toThrow(
    'too many requests',
  )
  expect(api.tree.resolve).toHaveBeenCalledTimes(1)
  expect(() =>
    structuralRoots(Array.from({ length: 1001 }, () => ({ path: 'a', kind: 'content' })) as never),
  ).toThrow('Too many conflicts')
})
