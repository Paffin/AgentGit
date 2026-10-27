// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import {
  bundleCommands,
  finishMigration,
  migrationWarnings,
  prepareMigration,
  readMigration,
} from './migration'
import type { MigrationReport } from './migration'

const transport = vi.hoisted(() => ({ token: 'first-token', fetch: vi.fn(), expired: vi.fn() }))
vi.mock('./instance', async () => {
  const { ApiClient } = await import('./client')
  return {
    client: new ApiClient({
      getToken: () => transport.token,
      fetch: transport.fetch,
      maxRetries: 1,
      onUnauthenticated: () => {
        transport.token = ''
        transport.expired()
      },
      sleep: async () => {},
    }),
  }
})

describe('migration request ownership', () => {
  it('does not end a new session when an old pinned request receives 401', async () => {
    transport.token = 'new-session'
    transport.expired.mockClear()
    transport.fetch.mockResolvedValue(
      new Response(JSON.stringify({ error: { code: 'unauthenticated', message: 'Expired' } }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      }),
    )
    await expect(readMigration('pending', { token: 'old-session' })).rejects.toMatchObject({
      status: 401,
    })
    expect(transport.token).toBe('new-session')
    expect(transport.expired).not.toHaveBeenCalled()
  })

  it.each(['read', 'prepare', 'finish'] as const)(
    'keeps the explicit credential for %s retries',
    async (operation) => {
      transport.token = 'first-token'
      const credentials: (string | null)[] = []
      transport.fetch.mockReset().mockImplementation(async (_url, init) => {
        credentials.push(new Headers(init.headers).get('authorization'))
        if (credentials.length === 1) {
          transport.token = 'second-token'
          return new Response(
            JSON.stringify({ error: { code: 'unavailable', message: 'Retry' } }),
            { status: 503, headers: { 'content-type': 'application/json' } },
          )
        }
        return new Response('{}', { headers: { 'content-type': 'application/json' } })
      })
      const options = { token: 'first-token' }
      if (operation === 'read') await readMigration('pending', options)
      else if (operation === 'prepare')
        await prepareMigration(
          'team/repo',
          new File(['bundle'], 'source.bundle'),
          'stable-key',
          options,
        )
      else await finishMigration('pending', false, options)
      expect(credentials).toEqual(['Bearer first-token', 'Bearer first-token'])
    },
  )
})

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
