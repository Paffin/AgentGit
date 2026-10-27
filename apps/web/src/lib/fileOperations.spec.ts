import { describe, expect, it } from 'vitest'
import { moveFile } from './fileOperations'

describe('moving a file', () => {
  const entry = {
    path: 'old',
    blob_id: 'exact-original-blob',
    size: 42,
    executable: true,
    file_type: 'symlink',
  }
  it('preserves the exact stored content, file type and executable bit', () => {
    expect(moveFile(entry, ' dir/new ')).toEqual([
      { path: 'old', op: 'delete' },
      {
        path: 'dir/new',
        op: 'put',
        blob_id: entry.blob_id,
        executable: true,
        file_type: 'symlink',
      },
    ])
  })
  it('rejects unchanged and unsafe paths before making changes', () => {
    for (const path of ['old', '../outside', '.ag/private', '/absolute', ''])
      expect(() => moveFile(entry, path)).toThrow()
  })
})
