// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import { ApiClient } from './client'
import { createApi } from './endpoints'

const invoke = (api: ReturnType<typeof createApi>, operation: string) => {
  const options = { token: 'initial-session', quietUnauthenticated: true }
  if (operation === 'revoke')
    return api.admin.revokeRole(
      { subject: 'user:alice', repo: 'acme/new', role: 'reader' },
      options,
    )
  if (operation === 'grant')
    return api.admin.grantRole({ subject: 'user:alice', repo: 'acme/new', role: 'writer' }, options)
  if (operation === 'workspace-create')
    return api.workspaces.create('acme/new', { name: 'edit', stream: 'main' }, options)
  if (operation === 'workspace-modify')
    return api.workspaces.modify(
      'acme/new',
      'edit',
      { changes: [], expected_head: 'base' },
      options,
    )
  if (operation === 'workspace-remove') return api.workspaces.remove('acme/new', 'edit', options)
  if (operation === 'proposal-create')
    return api.proposals.create(
      'acme/new',
      { stream: 'main', head: 'head', title: 'Edit' },
      options,
    )
  return operation === 'create'
    ? api.repos.create({ name: 'acme/new' }, options)
    : api.admin.setPolicy('acme/new', 'main', '{}', options)
}

describe('repository setup request ownership', () => {
  it.each([
    'create',
    'policy',
    'grant',
    'revoke',
    'workspace-create',
    'workspace-modify',
    'workspace-remove',
    'proposal-create',
  ])('keeps the initial session during a %s retry', async (operation) => {
    let token = 'initial-session'
    const credentials: (string | null)[] = []
    const fetchMock = vi.fn<typeof fetch>(async (_input, init) => {
      credentials.push(new Headers(init?.headers).get('authorization'))
      if (credentials.length === 1) {
        token = 'new-session'
        return new Response(JSON.stringify({ error: { code: 'unavailable', message: 'Retry' } }), {
          status: 503,
          headers: { 'content-type': 'application/json' },
        })
      }
      return new Response('{}', { headers: { 'content-type': 'application/json' } })
    })
    const client = new ApiClient({
      getToken: () => token,
      fetch: fetchMock,
      maxRetries: 1,
      sleep: async () => {},
    })
    await invoke(createApi(client), operation)
    expect(credentials).toEqual(['Bearer initial-session', 'Bearer initial-session'])
  })

  it.each([
    'create',
    'policy',
    'grant',
    'revoke',
    'workspace-create',
    'workspace-modify',
    'workspace-remove',
    'proposal-create',
  ])('does not reject a new session when an old %s request receives 401', async (operation) => {
    const rejected = vi.fn()
    const client = new ApiClient({
      getToken: () => 'new-session',
      onUnauthenticated: rejected,
      fetch: vi.fn<typeof fetch>().mockResolvedValue(
        new Response(JSON.stringify({ error: { code: 'unauthenticated', message: 'Expired' } }), {
          status: 401,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    })
    await expect(invoke(createApi(client), operation)).rejects.toMatchObject({ status: 401 })
    expect(rejected).not.toHaveBeenCalled()
  })
})
