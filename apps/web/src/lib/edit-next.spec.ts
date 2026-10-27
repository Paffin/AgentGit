import { describe, expect, it, vi } from 'vitest'
import type { Api } from '@/api/endpoints'
import type { Blocker, Conflict } from '@/api/types'
import {
  canResolveHere,
  deleteFile,
  pathProblem,
  proposeChange,
  putFile,
  toBase64,
  workspaceName,
} from './edit'
import { nextStep } from './nextstep'
import { mayDo, parseRoleGrants, rolesForRepo } from './roles'
import type { Action } from './roles'

describe('editing helpers', () => {
  it('encodes UTF-8 text as base64, including characters outside ASCII', () => {
    expect(toBase64('hello\n')).toBe('aGVsbG8K')
    expect(atob(toBase64('Grüße ✓'))).toBe(
      String.fromCharCode(...new TextEncoder().encode('Grüße ✓')),
    )
  })

  it('writes a put with the file mode, and a delete with nothing else', () => {
    expect(putFile('a/b.txt', 'x', true)).toEqual({
      path: 'a/b.txt',
      op: 'put',
      content: 'eA==',
      executable: true,
    })
    expect(deleteFile('a.txt')).toEqual({ path: 'a.txt', op: 'delete' })
  })

  it('explains what is wrong with a path, in words', () => {
    expect(pathProblem('')).toMatch(/Write the file’s path/)
    expect(pathProblem('/etc/passwd')).toMatch(/leading slash/)
    expect(pathProblem('docs/')).toMatch(/slash/)
    expect(pathProblem('a/../b')).toMatch(/“\.\.”/)
    expect(pathProblem('a//b')).toMatch(/empty parts/)
    expect(pathProblem('a\\b')).toMatch(/forward slashes/)
    expect(pathProblem('.git/config')).toMatch(/reserved/)
    expect(pathProblem('docs/intro.md')).toBeNull()
  })

  it('names drafts so that two people do not collide', () => {
    const a = workspaceName()
    const b = workspaceName()
    expect(a).toMatch(/^web-[0-9a-f]{10}$/)
    expect(a).not.toBe(b)
  })

  it('only offers to resolve here what a person can settle by choosing text', () => {
    for (const k of ['content', 'both_added', 'delete_modify']) expect(canResolveHere(k)).toBe(true)
    for (const k of [
      'binary',
      'type_change',
      'rename_rename',
      'directory_rename_ambiguous',
      'too_large',
    ])
      expect(canResolveHere(k)).toBe(false)
  })
})

describe('proposeChange', () => {
  const fakeApi = (over: { modify?: () => Promise<unknown> } = {}) => {
    const calls: string[] = []
    const api = {
      workspaces: {
        create: vi.fn(async (_r: string, b: { name: string }) => {
          calls.push(`create ${b.name}`)
          return { head: 'h0', base: 'h0', name: b.name }
        }),
        modify: vi.fn(over.modify ?? (async () => ({ head: 'h1' }))),
        remove: vi.fn(async () => {
          calls.push('remove')
          return {}
        }),
      },
      proposals: {
        create: vi.fn(async (_r: string, b: { head: string; title: string }) => {
          calls.push(`propose ${b.head}`)
          return { id: 'prop_1', title: b.title }
        }),
      },
    }
    return { api: api as unknown as Api, calls, raw: api }
  }

  it('opens a draft, applies the edit as one version, proposes it, and removes the draft', async () => {
    const { api, calls, raw } = fakeApi()
    const p = await proposeChange(api, {
      repo: 'acme/app',
      stream: 'main',
      fromRev: 'main',
      changes: [putFile('README.md', 'hi')],
      title: 'Say hi',
    })
    expect(p.id).toBe('prop_1')
    expect(calls[0]).toMatch(/^create web-/)
    expect(calls.slice(1)).toEqual(['propose h1', 'remove'])
    // the edit starts from what was on screen, and is refused if the draft moved meanwhile
    expect(raw.workspaces.create).toHaveBeenCalledWith(
      'acme/app',
      expect.objectContaining({ stream: 'main', from_rev: 'main' }),
    )
    expect(raw.workspaces.modify).toHaveBeenCalledWith(
      'acme/app',
      expect.stringMatching(/^web-/),
      expect.objectContaining({ expected_head: 'h0', message: 'Say hi' }),
    )
  })

  it('removes the draft when the edit is refused, and shows the real error', async () => {
    const { api, calls } = fakeApi({
      modify: async () => {
        throw new Error('head moved')
      },
    })
    await expect(
      proposeChange(api, { repo: 'a/b', stream: 'main', changes: [], title: 't' }),
    ).rejects.toThrow('head moved')
    expect(calls.at(-1)).toBe('remove')
  })

  it('refuses a changed starting point before writing and removes the unused workspace', async () => {
    const { api, calls, raw } = fakeApi()
    await expect(
      proposeChange(api, {
        repo: 'a/b',
        stream: 'main',
        expectedBase: '',
        changes: [putFile('new.txt', 'mine')],
        title: 'new',
      }),
    ).rejects.toThrow('branch changed')
    expect(raw.workspaces.modify).not.toHaveBeenCalled()
    expect(raw.proposals.create).not.toHaveBeenCalled()
    expect(calls.at(-1)).toBe('remove')
  })
})

describe('nextStep: one thing to do first', () => {
  const b = (code: string, detail: Record<string, unknown> = {}): Blocker => ({
    code,
    message: code,
    detail,
  })
  const conflict = { path: 'a', kind: 'content', message: '' } as Conflict
  const may = (roles: string[]) => {
    const have = rolesForRepo(parseRoleGrants(roles), 'acme/app')
    return (a: Action) => mayDo(have, a)
  }
  const reviewer = { may: may(['acme/*:reviewer']), isAuthor: false }
  const integrator = { may: may(['acme/*:integrator']), isAuthor: false }
  const authorWriter = { may: may(['acme/*:writer']), isAuthor: true }

  it('puts a conflict before everything, because it changes the files the rest depends on', () => {
    expect(nextStep([b('missing_validation')], [conflict], authorWriter)).toBe(
      'Resolve the conflict with the branch.',
    )
    expect(nextStep([], [conflict], reviewer)).toMatch(/author needs to resolve/)
  })

  it('then a failed check, a missing check, a missing record, approvals, a reservation', () => {
    expect(
      nextStep([b('failed_validation'), b('insufficient_approvals')], [], authorWriter),
    ).toMatch(/Fix the failed check/)
    expect(nextStep([b('missing_validation')], [], integrator)).toBe('Run the checks.')
    expect(nextStep([b('missing_validation')], [], reviewer)).toMatch(
      /Ask someone with the writer or integrator role/,
    )
    expect(nextStep([b('missing_provenance')], [], reviewer)).toMatch(/signed record/)
    expect(nextStep([b('insufficient_approvals', { have: 0, required: 2 })], [], reviewer)).toBe(
      'Get 2 more approvals from a reviewer. You can review it.',
    )
    expect(
      nextStep([b('insufficient_approvals', { have: 0, required: 1 })], [], authorWriter),
    ).toMatch(/You cannot approve your own change/)
    expect(nextStep([b('lease')], [], integrator)).toMatch(/reservation key/)
  })

  it('says to land it only to someone who may, and names who otherwise', () => {
    expect(nextStep([], [], integrator)).toBe('Land it.')
    expect(nextStep([], [], reviewer)).toMatch(/integrator role/)
  })
})
