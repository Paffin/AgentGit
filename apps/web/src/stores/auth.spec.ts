import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { ApiError } from '@/api/errors'
import type { Principal } from '@/api/types'

const whoami = vi.fn()
const capabilities = vi.fn()
const setApiToken = vi.fn()
let unauthenticated: (() => void) | null = null

vi.mock('@/api/instance', () => ({
  api: { whoami: (...a: unknown[]) => whoami(...a), capabilities: () => capabilities() },
  client: {},
  setApiToken: (t: string | null) => setApiToken(t),
  onUnauthenticated: (fn: () => void) => {
    unauthenticated = fn
    return () => undefined
  },
}))

import { TOKEN_KEY, cleanToken, useAuth } from './auth'

const principal = (over: Partial<Principal> = {}): Principal => ({
  identity: 'user:alice',
  kind: 'user',
  method: 'token',
  credential_id: 'cred_1',
  expires_at_ms: 0,
  roles: ['acme/*:writer', 'acme/web:reviewer'],
  scope: { repos: [], streams: [], operations: [], run_id: '' },
  ...over,
})

beforeEach(() => {
  setActivePinia(createPinia())
  whoami.mockReset()
  capabilities
    .mockReset()
    .mockResolvedValue({ protocol_version: 'agwp.v1', auth_methods: ['bearer'] })
  setApiToken.mockReset()
})

describe('cleanToken', () => {
  it('trims, drops a Bearer prefix and removes embedded whitespace from pasted tokens', () => {
    expect(cleanToken('  agt_abcdef123456  \n')).toBe('agt_abcdef123456')
    expect(cleanToken('Bearer agt_abcdef123456')).toBe('agt_abcdef123456')
    expect(cleanToken('agt_abc def\n123456')).toBe('agt_abcdef123456')
  })
  it('rejects what could not travel in a header', () => {
    expect(cleanToken('')).toBeNull()
    expect(cleanToken('short')).toBeNull()
    expect(cleanToken('agt_ünïcode_token_here')).toBeNull()
  })
})

describe('sign in', () => {
  it('checks the token with the server, keeps it in sessionStorage only, and reports who you are', async () => {
    whoami.mockResolvedValue(principal())
    const auth = useAuth()
    const who = await auth.signIn('  agt_goodtoken_0123456789 ')
    expect(whoami).toHaveBeenCalledWith('agt_goodtoken_0123456789')
    expect(who.identity).toBe('user:alice')
    expect(auth.isAuthenticated).toBe(true)
    expect(setApiToken).toHaveBeenCalledWith('agt_goodtoken_0123456789')
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBe('agt_goodtoken_0123456789')
    expect(window.localStorage.length).toBe(0) // never localStorage
    expect(JSON.stringify(window.localStorage)).not.toContain('agt_')
  })

  it('a bad token leaves no trace', async () => {
    whoami.mockRejectedValue(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'nope' }),
    )
    const auth = useAuth()
    await expect(auth.signIn('agt_badtoken_0123456789')).rejects.toMatchObject({
      code: 'unauthenticated',
    })
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.token).toBeNull()
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBeNull()
    expect(setApiToken).not.toHaveBeenCalled()
  })

  it('rejects an implausible token without calling the server', async () => {
    const auth = useAuth()
    await expect(auth.signIn('x')).rejects.toMatchObject({ code: 'invalid_argument' })
    expect(whoami).not.toHaveBeenCalled()
  })
})

describe('session lifecycle', () => {
  it('resumes a saved session', async () => {
    window.sessionStorage.setItem(TOKEN_KEY, 'agt_saved_token_0123456')
    whoami.mockResolvedValue(principal())
    const auth = useAuth()
    await auth.restore()
    expect(auth.isAuthenticated).toBe(true)
    expect(setApiToken).toHaveBeenCalledWith('agt_saved_token_0123456')
  })

  it('drops a saved token the server no longer accepts', async () => {
    window.sessionStorage.setItem(TOKEN_KEY, 'agt_stale_token_0123456')
    whoami.mockRejectedValue(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'expired' }),
    )
    const auth = useAuth()
    await auth.restore()
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.endedBecause).toBe('rejected')
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBeNull()
  })

  it('keeps the saved token when the server is merely unreachable', async () => {
    window.sessionStorage.setItem(TOKEN_KEY, 'agt_saved_token_0123456')
    whoami.mockRejectedValue(
      new ApiError({ status: 0, code: 'network_error', message: 'down', retryable: true }),
    )
    const auth = useAuth()
    await expect(auth.restore()).rejects.toMatchObject({ code: 'network_error' })
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBe('agt_saved_token_0123456')
  })

  it('signs out and forgets everything', async () => {
    whoami.mockResolvedValue(principal())
    const auth = useAuth()
    await auth.signIn('agt_goodtoken_0123456789')
    auth.signOut()
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.token).toBeNull()
    expect(auth.endedBecause).toBe('signed-out')
    expect(setApiToken).toHaveBeenLastCalledWith(null)
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBeNull()
  })

  it('ends the session when any request is answered 401', async () => {
    whoami.mockResolvedValue(principal())
    const auth = useAuth()
    await auth.signIn('agt_goodtoken_0123456789')
    unauthenticated?.()
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.endedBecause).toBe('expired')
  })

  it('ends the session at expiry and warns two minutes before', async () => {
    vi.useFakeTimers()
    const now = Date.now()
    whoami.mockResolvedValue(principal({ expires_at_ms: now + 5 * 60_000 }))
    const auth = useAuth()
    await auth.signIn('agt_goodtoken_0123456789')
    expect(auth.expiryWarned).toBe(false)
    await vi.advanceTimersByTimeAsync(3 * 60_000 + 1000)
    expect(auth.expiryWarned).toBe(true)
    expect(auth.isAuthenticated).toBe(true)
    await vi.advanceTimersByTimeAsync(2 * 60_000)
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.endedBecause).toBe('expired')
  })
})

describe('what the caller may do (hints)', () => {
  it('derives roles per repository from the whoami patterns', async () => {
    whoami.mockResolvedValue(principal())
    const auth = useAuth()
    await auth.signIn('agt_goodtoken_0123456789')
    expect([...auth.rolesFor('acme/web')].sort()).toEqual(['reviewer', 'writer'])
    expect(auth.can('acme/web', 'review')).toBe(true)
    expect(auth.can('acme/web', 'integrate')).toBe(false)
    expect(auth.can('acme/api', 'review')).toBe(false)
    expect(auth.hasRole('acme/web', 'admin')).toBe(false)
    expect(auth.isAdminAnywhere).toBe(false)
  })
  it('knows an administrator', async () => {
    whoami.mockResolvedValue(principal({ roles: ['*:admin'] }))
    const auth = useAuth()
    await auth.signIn('agt_goodtoken_0123456789')
    expect(auth.isAdminAnywhere).toBe(true)
    expect(auth.can('any/repo', 'integrate')).toBe(true)
  })
})
