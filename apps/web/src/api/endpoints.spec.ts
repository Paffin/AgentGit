// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import { ApiClient } from './client'
import { createApi } from './endpoints'

const invoke = (api: ReturnType<typeof createApi>, operation: string) => {
  const options = { token: 'initial-session', quietUnauthenticated: true }
  if (operation === 'grant')
    return api.admin.grantRole({ subject: 'user:alice', repo: 'acme/new', role: 'writer' }, options)
  return operation === 'create'
    ? api.repos.create({ name: 'acme/new' }, options)
    : api.admin.setPolicy('acme/new', 'main', '{}', options)
}

describe('repository setup request ownership', () => {
  it.each(['create', 'policy', 'grant'])(
    'keeps the initial session during a %s retry',
    async (operation) => {
      let token = 'initial-session'
      const credentials: (string | null)[] = []
      const fetchMock = vi.fn<typeof fetch>(async (_input, init) => {
        credentials.push(new Headers(init?.headers).get('authorization'))
        if (credentials.length === 1) {
          token = 'new-session'
          return new Response(
            JSON.stringify({ error: { code: 'unavailable', message: 'Retry' } }),
            { status: 503, headers: { 'content-type': 'application/json' } },
          )
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
    },
  )

  it.each(['create', 'policy', 'grant'])(
    'does not reject a new session when an old %s request receives 401',
    async (operation) => {
      const rejected = vi.fn()
      const client = new ApiClient({
        getToken: () => 'new-session',
        onUnauthenticated: rejected,
        fetch: vi
          .fn<typeof fetch>()
          .mockResolvedValue(
            new Response(
              JSON.stringify({ error: { code: 'unauthenticated', message: 'Expired' } }),
              { status: 401, headers: { 'content-type': 'application/json' } },
            ),
          ),
      })
      await expect(invoke(createApi(client), operation)).rejects.toMatchObject({ status: 401 })
      expect(rejected).not.toHaveBeenCalled()
    },
  )
})
