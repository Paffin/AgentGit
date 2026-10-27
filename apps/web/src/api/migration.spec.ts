import { describe, expect, it } from 'vitest'
import { bundleCommands, migrationWarnings } from './migration'
import type { MigrationReport } from './migration'

describe('migration guidance', () => {
  it('keeps the source a shell-quoted argument and does not allow credentials or local protocols', () => {
    expect(bundleCommands('ssh://git@gitlab.example/team/app.git')).toContain(
      "'ssh://git@gitlab.example/team/app.git'",
    )
    expect(bundleCommands('https://gitlab.example/team/app.git')).toContain(
      "-- 'https://gitlab.example/team/app.git'",
    )
    for (const source of [
      'file:///etc/passwd',
      'https://secret@github.com/a/b',
      'https://a:secret@github.com/a/b',
    ]) {
      expect(() => bundleCommands(source)).toThrow()
    }
    expect(bundleCommands("https://github.com/team/a'b.git")).toContain("a'\\''b.git")
  })
  it('makes every known content limitation visible', () => {
    const report: MigrationReport = {
      default_branch: 'main',
      commits: 1,
      blobs: 1,
      branches: [],
      tags: [],
      skipped_paths: [['x', 'reason']],
      skipped_submodules: 2,
      lossy_commits: 3,
      lfs_pointers: 4,
      other_refs: ['refs/notes/x'],
    }
    expect(migrationWarnings(report)).toHaveLength(5)
    expect(
      migrationWarnings({
        ...report,
        skipped_paths: [],
        skipped_submodules: 0,
        lossy_commits: 0,
        lfs_pointers: 0,
        other_refs: [],
      }),
    ).toEqual([])
  })
})
