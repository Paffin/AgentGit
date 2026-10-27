import { describe, expect, it, vi } from 'vitest'
import { uploadFiles, uploadChanges, checkUploads, UPLOAD_LIMIT } from './uploads'
import type { Api } from '@/api/endpoints'

describe('browser file uploads', () => {
  it('accepts the exact count and byte limits, and rejects the next item', () => {
    const files = Array.from({ length: 100 }, (_, i) => new File([''], `${i}.txt`))
    expect(uploadFiles(files, '')).toHaveLength(100)
    expect(() => uploadFiles([...files, new File([''], 'extra')], '')).toThrow('at most 100')
    expect(uploadFiles([new File([new Uint8Array(UPLOAD_LIMIT)], 'exact')], '')).toHaveLength(1)
  })
  it('recognizes empty branches and absent files as new non-executable uploads', async () => {
    const resolve = vi.fn().mockResolvedValue({ kind: 'missing', entry: null })
    const api = { tree: { resolve } } as unknown as Api
    const files = uploadFiles([new File(['new'], 'new.txt')], '')
    expect(await checkUploads(api, 'team/repo', '', files)).toMatchObject([
      { replaces: false, executable: false },
    ])
    expect(resolve).not.toHaveBeenCalled()
    expect(await checkUploads(api, 'team/repo', 'base', files)).toMatchObject([
      { replaces: false, executable: false },
    ])
  })
  it('keeps binary bytes and destination paths exactly', async () => {
    const file = new File([new Uint8Array([0, 255, 13, 10])], 'asset.bin')
    const selected = uploadFiles([file], 'assets/')
    expect(selected[0]!.path).toBe('assets/asset.bin')
    const changes = await uploadChanges(
      selected.map((item) => ({ ...item, replaces: false, executable: true })),
    )
    expect(changes).toEqual([
      {
        path: 'assets/asset.bin',
        op: 'put',
        content: 'AP8NCg==',
        file_type: 'regular',
        executable: true,
      },
    ])
  })
  it('refuses duplicate destinations, unsafe folders and oversized batches', () => {
    const file = new File(['x'], 'one.txt')
    expect(() => uploadFiles([file, file], '')).toThrow('same destination')
    expect(() => uploadFiles([file], '../outside')).toThrow()
    expect(() => uploadFiles([new File([new Uint8Array(UPLOAD_LIMIT + 1)], 'large')], '')).toThrow(
      '4 MiB',
    )
  })
  it('checks existing paths against the pinned revision and preserves executable mode', async () => {
    const resolve = vi.fn().mockResolvedValue({ kind: 'regular', entry: { executable: true } })
    const api = { tree: { resolve } } as unknown as Api
    const checked = await checkUploads(
      api,
      'team/repo',
      'fixed-revision',
      uploadFiles([new File(['x'], 'run.sh')], ''),
    )
    expect(resolve).toHaveBeenCalledWith('team/repo', 'fixed-revision', 'run.sh')
    expect(checked[0]).toMatchObject({ replaces: true, executable: true })
    resolve.mockResolvedValue({ kind: 'dir' })
    await expect(checkUploads(api, 'team/repo', 'fixed-revision', checked)).rejects.toThrow(
      'already a folder',
    )
  })
})
