import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { ApiError } from '@/api/errors'
import type { AuthConfig, Principal } from '@/api/types'
import type { Flow } from '@/lib/oidc'

const whoami = vi.fn()
const authConfig = vi.fn()
const setApiToken = vi.fn()

vi.mock('@/api/instance', () => ({
  api: {
    whoami: (...a: unknown[]) => whoami(...a),
    authConfig: () => authConfig(),
    capabilities: () =>
      Promise.resolve({ protocol_version: 'agwp.v1', auth_methods: ['bearer', 'oidc'] }),
  },
  client: {},
  setApiToken: (t: string | null) => setApiToken(t),
  onUnauthenticated: () => () => undefined,
}))

import { FLOW_KEY, SSO_KEY, TOKEN_KEY, useAuth } from './auth'

const ISSUER = 'https://idp.example.com/realms/acme'
const config: AuthConfig = {
  mode: 'oidc',
  providers: [
    {
      name: 'Acme SSO',
      issuer: ISSUER,
      client_id: 'agentgit-web',
      scopes: ['openid', 'profile'],
      authorization_endpoint: 'https://idp.example.com/auth',
      token_endpoint: 'https://idp.example.com/token',
      end_session_endpoint: 'https://idp.example.com/logout',
      token: 'id_token',
    },
  ],
}
const b64u = (s: string) => btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
const jwt = (claims: Record<string, unknown>) =>
  `${b64u('{"alg":"EdDSA"}')}.${b64u(JSON.stringify(claims))}.${b64u('sig')}`
const inSecs = (s: number) => Math.floor(Date.now() / 1000) + s

const flow = (over: Partial<Flow> = {}): Flow => ({
  state: 'state-1',
  nonce: 'nonce-1',
  verifier: 'verifier-1',
  issuer: ISSUER,
  clientId: 'agentgit-web',
  endpoints: {
    authorization: 'https://idp.example.com/auth',
    token: 'https://idp.example.com/token',
    endSession: 'https://idp.example.com/logout',
  },
  next: '/repos/acme/web',
  silent: false,
  startedAt: Date.now(),
  ...over,
})

const principal = (over: Partial<Principal> = {}): Principal => ({
  identity: 'user:dana',
  kind: 'user',
  method: 'oidc',
  credential_id: 'c',
  expires_at_ms: Date.now() + 600_000,
  roles: ['acme/*:reader'],
  scope: { repos: [], streams: [], operations: [], run_id: '' },
  ...over,
})

function tokenResponse(over: Record<string, unknown> = {}) {
  return new Response(
    JSON.stringify({
      id_token: jwt({ iss: ISSUER, aud: 'agentgit-web', exp: inSecs(600), nonce: 'nonce-1' }),
      access_token: 'opaque',
      refresh_token: 'r1',
      expires_in: 600,
      ...over,
    }),
    { status: 200, headers: { 'Content-Type': 'application/json' } },
  )
}

let fetchMock: ReturnType<typeof vi.fn>
const assign = vi.fn()

beforeEach(() => {
  setActivePinia(createPinia())
  whoami.mockReset().mockResolvedValue(principal())
  authConfig.mockReset().mockResolvedValue(config)
  setApiToken.mockReset()
  assign.mockReset()
  fetchMock = vi.fn().mockResolvedValue(tokenResponse())
  vi.stubGlobal('fetch', fetchMock)
  // happy-dom lets us replace the navigation hook
  Object.defineProperty(window, 'location', {
    configurable: true,
    value: { origin: 'https://ag.example.com', assign, search: '', pathname: '/auth/callback' },
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

const start = (f: Flow) => window.sessionStorage.setItem(FLOW_KEY, JSON.stringify(f))

describe('starting a redirect sign-in', () => {
  it('keeps only the flow state, and sends the browser to the provider', async () => {
    const auth = useAuth()
    await auth.loadAuthConfig()
    await auth.signInWithSso(0, '/repos/acme/web')
    expect(assign).toHaveBeenCalledTimes(1)
    const url = new URL(assign.mock.calls[0]?.[0] as string)
    expect(url.origin + url.pathname).toBe('https://idp.example.com/auth')
    const saved = JSON.parse(window.sessionStorage.getItem(FLOW_KEY) ?? '{}') as Flow
    expect(saved.state).toBe(url.searchParams.get('state'))
    expect(saved.next).toBe('/repos/acme/web')
    expect(url.searchParams.get('code_challenge_method')).toBe('S256')
  })

  it('says so when the provider is not configured, and when the server does not offer redirect sign-in', async () => {
    const auth = useAuth()
    await expect(auth.signInWithSso(3, '/')).rejects.toThrowError(/not configured/)
    authConfig.mockRejectedValue(new ApiError({ status: 0, code: 'unavailable', message: 'down' }))
    const other = (setActivePinia(createPinia()), useAuth())
    expect((await other.loadAuthConfig()).mode).toBe('token')
  })
})

describe('finishing it', () => {
  it('trades the code, asks the server who this is, and keeps the token in memory only', async () => {
    start(flow())
    const auth = useAuth()
    const next = await auth.completeSso('?code=the-code&state=state-1')
    expect(next).toBe('/repos/acme/web')
    expect(auth.isAuthenticated).toBe(true)
    expect(auth.method).toBe('oidc')
    expect(auth.identity).toBe('user:dana')
    const bearer = whoami.mock.calls[0]?.[0] as string
    expect(bearer).toMatch(/^eyJ/)
    expect(setApiToken).toHaveBeenCalledWith(bearer)
    // The flow is consumed, and not a character of any token reaches storage.
    expect(window.sessionStorage.getItem(FLOW_KEY)).toBeNull()
    expect(window.sessionStorage.getItem(TOKEN_KEY)).toBeNull()
    expect(JSON.stringify({ ...window.sessionStorage, ...window.localStorage })).not.toContain(
      'eyJ',
    )
    expect(JSON.parse(window.sessionStorage.getItem(SSO_KEY) ?? '{}')).toEqual({
      issuer: ISSUER,
      clientId: 'agentgit-web',
    })
  })

  it('a flow works once: a replay of the same callback finds nothing in progress', async () => {
    start(flow())
    const auth = useAuth()
    await auth.completeSso('?code=the-code&state=state-1')
    auth.signOut()
    await expect(auth.completeSso('?code=the-code&state=state-1')).rejects.toThrowError(
      /no sign-in in progress/,
    )
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('ignores a wrong state without asking the provider for tokens', async () => {
    start(flow())
    const auth = useAuth()
    await expect(auth.completeSso('?code=c&state=forged')).rejects.toThrowError(/did not match/)
    expect(fetchMock).not.toHaveBeenCalled()
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.endedBecause).toBe('sso')
    expect(auth.ssoMessage).toMatch(/did not match/)
    expect(window.sessionStorage.getItem(FLOW_KEY)).toBeNull()
  })

  it('refuses a flow that is too old', async () => {
    start(flow({ startedAt: Date.now() - 11 * 60_000 }))
    const auth = useAuth()
    await expect(auth.completeSso('?code=c&state=state-1')).rejects.toThrowError(
      /no sign-in in progress/,
    )
  })

  it('explains a token the server does not accept', async () => {
    start(flow())
    whoami.mockRejectedValue(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'nope' }),
    )
    const auth = useAuth()
    await expect(auth.completeSso('?code=c&state=state-1')).rejects.toThrowError(
      /did not accept the ID token/,
    )
    expect(auth.isAuthenticated).toBe(false)
    expect(setApiToken).not.toHaveBeenCalledWith(expect.stringMatching(/^eyJ/))
  })
})

describe('after sign-in', () => {
  it('signing out also leaves for the provider’s logout, with the ID token as hint', async () => {
    start(flow())
    const auth = useAuth()
    await auth.completeSso('?code=c&state=state-1')
    const url = auth.signOut()
    const q = new URL(url ?? '').searchParams
    expect(url).toMatch(/^https:\/\/idp\.example\.com\/logout\?/)
    expect(q.get('id_token_hint')).toMatch(/^eyJ/)
    expect(q.get('post_logout_redirect_uri')).toBe('https://ag.example.com/signin')
    expect(auth.isAuthenticated).toBe(false)
    expect(window.sessionStorage.getItem(SSO_KEY)).toBeNull()
  })

  it('renews the token silently before it expires, using the refresh token', async () => {
    vi.useFakeTimers()
    start(flow())
    const soon = jwt({ iss: ISSUER, aud: 'agentgit-web', exp: inSecs(90), nonce: 'nonce-1' })
    fetchMock.mockResolvedValueOnce(tokenResponse({ id_token: soon }))
    whoami.mockResolvedValue(principal({ expires_at_ms: Date.now() + 90_000 }))
    const auth = useAuth()
    await auth.completeSso('?code=c&state=state-1')
    expect(auth.expiryWarned).toBe(false) // a session that renews itself warns nobody

    const renewed = jwt({ iss: ISSUER, aud: 'agentgit-web', exp: inSecs(900) })
    fetchMock.mockResolvedValueOnce(tokenResponse({ id_token: renewed, refresh_token: 'r2' }))
    whoami.mockResolvedValue(principal({ expires_at_ms: Date.now() + 900_000 }))
    await vi.advanceTimersByTimeAsync(31_000) // 90 s left, renew a minute early
    const grant = new URLSearchParams((fetchMock.mock.calls[1]?.[1] as RequestInit).body as string)
    expect(grant.get('grant_type')).toBe('refresh_token')
    expect(grant.get('refresh_token')).toBe('r1')
    expect(setApiToken).toHaveBeenLastCalledWith(renewed)
    expect(auth.isAuthenticated).toBe(true)
  })

  it('runs out with a warning when the provider gave no refresh token', async () => {
    vi.useFakeTimers()
    start(flow())
    const soon = jwt({ iss: ISSUER, aud: 'agentgit-web', exp: inSecs(60), nonce: 'nonce-1' })
    fetchMock.mockResolvedValueOnce(tokenResponse({ id_token: soon, refresh_token: undefined }))
    whoami.mockResolvedValue(principal({ expires_at_ms: Date.now() + 60_000 }))
    const auth = useAuth()
    await auth.completeSso('?code=c&state=state-1')
    expect(auth.expiryWarned).toBe(true)
    await vi.advanceTimersByTimeAsync(61_000)
    expect(auth.isAuthenticated).toBe(false)
    expect(auth.endedBecause).toBe('expired')
  })
})

describe('resuming after a reload', () => {
  it('asks the provider again, silently, from where the person was', async () => {
    window.sessionStorage.setItem(
      SSO_KEY,
      JSON.stringify({ issuer: ISSUER, clientId: 'agentgit-web' }),
    )
    const auth = useAuth()
    const waiting = auth.restore('/repos/acme/web/files')
    await vi.waitFor(() => expect(assign).toHaveBeenCalledTimes(1))
    const url = new URL(assign.mock.calls[0]?.[0] as string)
    expect(url.searchParams.get('prompt')).toBe('none')
    expect(JSON.parse(window.sessionStorage.getItem(FLOW_KEY) ?? '{}')).toMatchObject({
      next: '/repos/acme/web/files',
      silent: true,
    })
    void waiting // the page is leaving: the navigation is held until it does
  })

  it('does nothing without the marker, and drops a marker for a provider that is gone', async () => {
    const auth = useAuth()
    await auth.restore('/x')
    expect(assign).not.toHaveBeenCalled()
    window.sessionStorage.setItem(
      SSO_KEY,
      JSON.stringify({ issuer: 'https://gone.example.com', clientId: 'x' }),
    )
    setActivePinia(createPinia())
    await useAuth().restore('/x')
    expect(assign).not.toHaveBeenCalled()
    expect(window.sessionStorage.getItem(SSO_KEY)).toBeNull()
  })

  it('a silent request the provider cannot answer leaves a message, not a loop', async () => {
    start(flow({ silent: true }))
    const auth = useAuth()
    await expect(auth.completeSso('?error=login_required&state=state-1')).rejects.toThrowError(
      /session has ended/,
    )
    expect(auth.endedBecause).toBe('sso')
    expect(window.sessionStorage.getItem(SSO_KEY)).toBeNull()
  })
})
