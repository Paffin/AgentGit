import { describe, expect, it } from 'vitest'
import { conflictLabel, isDirectoryRenameConflict } from './conflict-label'

describe('conflictLabel', () => {
  it('prefers the server message', () => {
    expect(conflictLabel({ kind: 'content', message: 'content conflict (2 region(s))' })).toBe(
      'content conflict (2 region(s))',
    )
  })
  it('explains the directory rename codes when there is no message', () => {
    expect(conflictLabel({ kind: 'directory_rename_ambiguous' })).toMatch(/more than one place/)
    expect(conflictLabel({ kind: 'directory_rename_collision', message: '' })).toMatch(
      /already taken/,
    )
  })
  it('replaces every underscore of an unknown code', () => {
    expect(conflictLabel({ kind: 'some_future_code' })).toBe('some future code')
  })
  it('recognises directory rename conflicts', () => {
    expect(isDirectoryRenameConflict({ kind: 'directory_rename_collision' })).toBe(true)
    expect(isDirectoryRenameConflict({ kind: 'rename_rename' })).toBe(false)
  })
})
