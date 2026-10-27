// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import {
  OidcError,
  authorizationUrl,
  beginFlow,
  checkCallback,
  checkIdToken,
  codeChallenge,
  discover,
  exchangeCode,
  jwtClaims,
  logoutUrl,
  parseCallback,
  randomToken,
  redirectUri,
  refreshTokens,
  sameIssuer,
} from './oidc'
import type { Endpoints, Flow, Provider } from './oidc'

const b64u = (s: string) => btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
const jwt = (claims: Record<string, unknown>) =>
  `${b64u('{"alg":"EdDSA"}')}.${b64u(JSON.stringify(claims))}.${b64u('signature')}`

const provider: Provider = {
  name: 'Acme SSO',
  issuer: 'https://idp.example.com/realms/acme',
  client_id: 'agentgit-web',
  scopes: ['openid', 'profile'],
  token: 'id_token',
}
const endpoints: Endpoints = {
  authorization: 'https://idp.example.com/auth',
  token: 'https://idp.example.com/token',
  endSession: 'https://idp.example.com/logout',
}
const flow: Flow = {
  state: 'state-123',
  nonce: 'nonce-456',
  verifier: 'verifier-789',
  issuer: provider.issuer,
  clientId: provider.client_id,
  endpoints,
  next: '/repos/acme/web',
  silent: false,
  startedAt: 0,
}
const NOW = 1_800_000_000_000
const exp = (secs: number) => Math.floor(NOW / 1000) + secs

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('PKCE', () => {
  it('computes the S256 challenge of the RFC 7636 example', async () => {
    expect(await codeChallenge('dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk')).toBe(
      'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
    )
  })

  it('makes unguessable URL-safe values', () => {
    const a = randomToken(32)
    expect(a).toMatch(/^[A-Za-z0-9_-]{43}$/)
    expect(randomToken(32)).not.toBe(a)
  })
})

describe('the authorization request', () => {
  it('asks for a code, with PKCE, state and nonce, and never sends a secret', () => {
    const url = new URL(
      authorizationUrl(provider, endpoints, {
        state: 's',
        nonce: 'n',
        challenge: 'c',
        redirect: redirectUri('https://ag.example.com'),
      }),
    )
    const q = url.searchParams
    expect(url.origin + url.pathname).toBe('https://idp.example.com/auth')
    expect(q.get('response_type')).toBe('code')
    expect(q.get('client_id')).toBe('agentgit-web')
    expect(q.get('redirect_uri')).toBe('https://ag.example.com/auth/callback')
    expect(q.get('scope')).toBe('openid profile')
    expect(q.get('code_challenge')).toBe('c')
    expect(q.get('code_challenge_method')).toBe('S256')
    expect([q.get('state'), q.get('nonce')]).toEqual(['s', 'n'])
    expect(q.has('prompt')).toBe(false)
    expect(q.has('client_secret')).toBe(false)
  })

  it('a silent request says prompt=none', () => {
    const url = authorizationUrl(provider, endpoints, {
      state: 's',
      nonce: 'n',
      challenge: 'c',
      redirect: 'https://ag.example.com/auth/callback',
      silent: true,
    })
    expect(new URL(url).searchParams.get('prompt')).toBe('none')
  })

  it('begins a flow whose challenge matches its verifier', async () => {
    const full: Provider = {
      ...provider,
      authorization_endpoint: endpoints.authorization,
      token_endpoint: endpoints.token,
    }
    const { flow: f, url } = await beginFlow(full, '/x', 'https://ag.example.com', { now: () => 5 })
    expect(f.startedAt).toBe(5)
    expect(f.next).toBe('/x')
    expect(new URL(url).searchParams.get('code_challenge')).toBe(await codeChallenge(f.verifier))
    expect(new URL(url).searchParams.get('state')).toBe(f.state)
    expect(f.verifier).not.toBe(f.state)
  })
})

describe('discovery', () => {
  it('uses the endpoints the server names, without asking the provider', async () => {
    const fetchFn = vi.fn()
    const ep = await discover(
      {
        ...provider,
        authorization_endpoint: 'https://a.example.com/auth',
        token_endpoint: 'https://t.example.com/token',
      },
      fetchFn,
    )
    expect(ep).toEqual({
      authorization: 'https://a.example.com/auth',
      token: 'https://t.example.com/token',
      endSession: undefined,
    })
    expect(fetchFn).not.toHaveBeenCalled()
  })

  it('reads the provider’s configuration, without credentials, and checks its issuer', async () => {
    const fetchFn = vi.fn().mockResolvedValue(
      jsonResponse({
        issuer: `${provider.issuer}/`,
        authorization_endpoint: endpoints.authorization,
        token_endpoint: endpoints.token,
        end_session_endpoint: endpoints.endSession,
      }),
    )
    const ep = await discover(provider, fetchFn)
    expect(ep).toEqual(endpoints)
    const [url, init] = fetchFn.mock.calls[0] as [string, RequestInit]
    expect(url).toBe(`${provider.issuer}/.well-known/openid-configuration`)
    expect(init.credentials).toBe('omit')
  })

  it('refuses a provider that describes another issuer', async () => {
    const fetchFn = vi.fn().mockResolvedValue(
      jsonResponse({
        issuer: 'https://evil.example.com',
        authorization_endpoint: endpoints.authorization,
        token_endpoint: endpoints.token,
      }),
    )
    await expect(discover(provider, fetchFn)).rejects.toMatchObject({ code: 'discovery' })
  })

  it('explains an unreachable provider, mentioning CORS', async () => {
    const fetchFn = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'))
    const e = await discover(provider, fetchFn).catch((x: unknown) => x)
    expect(e).toBeInstanceOf(OidcError)
    expect((e as OidcError).message).toMatch(/CORS/)
  })

  it('refuses endpoints that are not https (loopback excepted)', async () => {
    const bad = {
      ...provider,
      authorization_endpoint: 'http://idp.example.com/auth',
      token_endpoint: 'https://idp.example.com/token',
    }
    await expect(discover(bad, vi.fn())).rejects.toMatchObject({ code: 'discovery' })
    const local = {
      ...provider,
      authorization_endpoint: 'http://127.0.0.1:9000/auth',
      token_endpoint: 'http://localhost:9000/token',
    }
    await expect(discover(local, vi.fn())).resolves.toBeTruthy()
  })
})

describe('the callback', () => {
  it('returns the code when the state matches', () => {
    expect(checkCallback(parseCallback('?code=abc&state=state-123'), flow)).toBe('abc')
  })

  it('ignores a response whose state differs, and one with no sign-in in progress', () => {
    expect(() => checkCallback(parseCallback('?code=abc&state=forged'), flow)).toThrowError(
      expect.objectContaining({ code: 'state' }),
    )
    expect(() => checkCallback(parseCallback('?code=abc'), flow)).toThrowError(
      expect.objectContaining({ code: 'state' }),
    )
    expect(() => checkCallback(parseCallback('?code=abc&state=state-123'), null)).toThrowError(
      /no sign-in in progress/,
    )
  })

  it('turns the provider’s errors into sentences', () => {
    expect(() =>
      checkCallback(parseCallback('?error=login_required&state=state-123'), flow),
    ).toThrowError(expect.objectContaining({ code: 'login_required' }))
    expect(() =>
      checkCallback(
        parseCallback('?error=access_denied&error_description=No%20thanks&state=state-123'),
        flow,
      ),
    ).toThrowError(/refused the sign-in: No thanks/)
    expect(() => checkCallback(parseCallback('?state=state-123'), flow)).toThrowError(
      /did not return an authorization code/,
    )
  })
})

describe('the ID token', () => {
  const ok = { iss: provider.issuer, aud: provider.client_id, exp: exp(600), nonce: 'nonce-456' }
  const check = (claims: Record<string, unknown>) =>
    checkIdToken(
      jwt(claims),
      { issuer: provider.issuer, clientId: provider.client_id, nonce: 'nonce-456' },
      NOW,
    )

  it('reads claims without trusting them', () => {
    expect(jwtClaims(jwt({ a: 1 }))).toEqual({ a: 1 })
    expect(jwtClaims('not a jwt')).toBeNull()
    expect(jwtClaims('a.!!!.c')).toBeNull()
  })

  it('accepts one for this issuer, client and nonce', () => {
    expect(() => check(ok)).not.toThrow()
    expect(() => check({ ...ok, aud: ['other', provider.client_id] })).not.toThrow()
  })

  it('refuses another issuer, another client, an expired token or another nonce', () => {
    expect(() => check({ ...ok, iss: 'https://evil.example.com' })).toThrowError(
      /different provider/,
    )
    expect(() => check({ ...ok, aud: 'someone-else' })).toThrowError(/different application/)
    expect(() => check({ ...ok, exp: exp(-3600) })).toThrowError(/expired/)
    expect(() => check({ ...ok, nonce: 'replayed' })).toThrowError(
      expect.objectContaining({ code: 'nonce' }),
    )
    expect(() => check({ ...ok, nonce: undefined })).toThrowError(
      expect.objectContaining({ code: 'nonce' }),
    )
  })

  it('compares issuers without caring about a trailing slash', () => {
    expect(sameIssuer('https://a.example.com/', 'https://a.example.com')).toBe(true)
    expect(sameIssuer('https://a.example.com', 'https://b.example.com')).toBe(false)
  })
})

describe('the token exchange', () => {
  const idToken = jwt({
    iss: provider.issuer,
    aud: provider.client_id,
    exp: exp(600),
    nonce: 'nonce-456',
  })

  it('sends the verifier and the redirect URI, as a public client, and picks the bearer', async () => {
    const fetchFn = vi.fn().mockResolvedValue(
      jsonResponse({
        id_token: idToken,
        access_token: 'opaque',
        refresh_token: 'r1',
        expires_in: 600,
      }),
    )
    const t = await exchangeCode(provider, flow, 'the-code', 'https://ag.example.com', fetchFn, NOW)
    const [url, init] = fetchFn.mock.calls[0] as [string, RequestInit]
    expect(url).toBe(endpoints.token)
    expect(init.method).toBe('POST')
    expect(init.credentials).toBe('omit')
    const body = new URLSearchParams(init.body as string)
    expect(Object.fromEntries(body)).toEqual({
      grant_type: 'authorization_code',
      code: 'the-code',
      redirect_uri: 'https://ag.example.com/auth/callback',
      client_id: 'agentgit-web',
      code_verifier: 'verifier-789',
    })
    expect(t.bearer).toBe(idToken)
    expect(t.refreshToken).toBe('r1')
    expect(t.expiresAtMs).toBe(exp(600) * 1000)
  })

  it('can present the access token instead', async () => {
    const access = jwt({ exp: exp(300) })
    const fetchFn = vi
      .fn()
      .mockResolvedValue(jsonResponse({ id_token: idToken, access_token: access }))
    const t = await exchangeCode(
      { ...provider, token: 'access_token' },
      flow,
      'c',
      'https://ag.example.com',
      fetchFn,
      NOW,
    )
    expect(t.bearer).toBe(access)
    expect(t.expiresAtMs).toBe(exp(300) * 1000)
  })

  it('reports what the provider refused', async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ error: 'invalid_grant', error_description: 'PKCE failed' }, 400),
      )
    await expect(
      exchangeCode(provider, flow, 'c', 'https://ag.example.com', fetchFn, NOW),
    ).rejects.toThrowError(/PKCE failed/)
  })

  it('explains a blocked request (no CORS) and a missing ID token', async () => {
    const blocked = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'))
    await expect(
      exchangeCode(provider, flow, 'c', 'https://ag.example.com', blocked, NOW),
    ).rejects.toThrowError(/CORS/)
    const none = vi.fn().mockResolvedValue(jsonResponse({ access_token: 'x' }))
    await expect(
      exchangeCode(provider, flow, 'c', 'https://ag.example.com', none, NOW),
    ).rejects.toThrowError(/ID token/)
  })

  it('rejects an ID token with the wrong nonce before anything is used', async () => {
    const wrong = jwt({
      iss: provider.issuer,
      aud: provider.client_id,
      exp: exp(600),
      nonce: 'other',
    })
    const fetchFn = vi.fn().mockResolvedValue(jsonResponse({ id_token: wrong }))
    await expect(
      exchangeCode(provider, flow, 'c', 'https://ag.example.com', fetchFn, NOW),
    ).rejects.toMatchObject({
      code: 'nonce',
    })
  })

  it('refreshes with the refresh token and keeps it when the provider does not rotate it', async () => {
    const next = jwt({ iss: provider.issuer, aud: provider.client_id, exp: exp(900) })
    const fetchFn = vi.fn().mockResolvedValue(jsonResponse({ id_token: next }))
    const t = await refreshTokens(provider, endpoints, 'r1', fetchFn, NOW)
    const body = new URLSearchParams(
      (fetchFn.mock.calls[0] as [string, RequestInit])[1].body as string,
    )
    expect(Object.fromEntries(body)).toEqual({
      grant_type: 'refresh_token',
      refresh_token: 'r1',
      client_id: 'agentgit-web',
    })
    expect(t.bearer).toBe(next)
    expect(t.refreshToken).toBe('r1')
  })
})

describe('logging out', () => {
  it('goes to the provider’s logout with the hint, and back to the sign-in page', () => {
    const url = logoutUrl(provider, endpoints, 'the.id.token', 'https://ag.example.com')
    const q = new URL(url ?? '').searchParams
    expect(q.get('id_token_hint')).toBe('the.id.token')
    expect(q.get('post_logout_redirect_uri')).toBe('https://ag.example.com/signin')
    expect(q.get('client_id')).toBe('agentgit-web')
  })

  it('has nowhere to go when the provider offers no logout', () => {
    expect(
      logoutUrl(provider, { ...endpoints, endSession: undefined }, 'x', 'https://ag.example.com'),
    ).toBeNull()
    expect(logoutUrl(provider, undefined, undefined, 'https://ag.example.com')).toBeNull()
  })
})
