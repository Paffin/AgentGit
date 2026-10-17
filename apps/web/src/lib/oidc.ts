// Redirect sign-in with an OpenID Connect provider: the authorization-code flow with PKCE (RFC 7636, S256), as a
// public client. Everything here is a pure function over `fetch` and `crypto`, so it is unit-tested without a
// browser; the auth store drives it.
//
// What the browser checks, because it is the only one who can: the `state` that comes back is the one it sent
// (against cross-site request forgery), the ID token carries the `nonce` it sent (against replay), names this
// issuer and this client, and has not expired. The signature is NOT checked here: the server does, with the
// provider's keys, on every request. A token the browser accepted but the server would not is simply refused at
// sign-in (`whoami`).
//
// No token is ever written to storage: only the short-lived flow state (verifier, state, nonce) survives the
// redirect, in sessionStorage, and it is deleted the moment the callback is handled.

/** One identity provider, as `GET /v1/auth/config` describes it. */
export interface Provider {
  name: string
  issuer: string
  client_id: string
  scopes: string[]
  authorization_endpoint?: string | null
  token_endpoint?: string | null
  end_session_endpoint?: string | null
  /** Which token is the bearer credential for this server. */
  token: 'id_token' | 'access_token'
}

export interface Endpoints {
  authorization: string
  token: string
  endSession?: string
}

/** What must survive the redirect to the provider and back. */
export interface Flow {
  state: string
  nonce: string
  verifier: string
  /** The provider's issuer and client id (to find it in the configuration again). */
  issuer: string
  clientId: string
  endpoints: Endpoints
  /** Where to go after signing in. */
  next: string
  /** The request was `prompt=none`: failing is expected and must not be retried. */
  silent: boolean
  startedAt: number
}

export interface Tokens {
  /** The credential to present to the ag-server. */
  bearer: string
  idToken?: string
  refreshToken?: string
  /** When the bearer expires (ms since the epoch), from its own `exp` claim; 0 if it has none we can read. */
  expiresAtMs: number
}

export type OidcErrorCode =
  | 'unsupported'
  | 'config'
  | 'discovery'
  | 'state'
  | 'nonce'
  | 'denied'
  | 'login_required'
  | 'token'
  | 'id_token'

/** A failure of the sign-in flow, with a message that can be shown to a person as is. */
export class OidcError extends Error {
  readonly code: OidcErrorCode
  constructor(code: OidcErrorCode, message: string) {
    super(message)
    this.name = 'OidcError'
    this.code = code
  }
}

const trimSlash = (s: string) => s.replace(/\/+$/, '')
export const sameIssuer = (a: string, b: string) => trimSlash(a) === trimSlash(b)

export function base64Url(bytes: Uint8Array): string {
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** Random, URL-safe, unguessable: 32 bytes is 43 characters of entropy for the verifier, state and nonce. */
export function randomToken(bytes = 32): string {
  const buf = new Uint8Array(bytes)
  globalThis.crypto.getRandomValues(buf)
  return base64Url(buf)
}

/** The PKCE code challenge of a verifier: BASE64URL(SHA-256(verifier)). */
export async function codeChallenge(verifier: string): Promise<string> {
  const subtle = globalThis.crypto?.subtle
  if (!subtle) {
    throw new OidcError(
      'unsupported',
      'Redirect sign-in needs a secure context: open this page over HTTPS (or on localhost).',
    )
  }
  const digest = await subtle.digest('SHA-256', new TextEncoder().encode(verifier))
  return base64Url(new Uint8Array(digest))
}

/** The redirect URI registered with the provider for this UI. */
export const redirectUri = (origin: string): string => `${origin}/auth/callback`

/** `http` is accepted for loopback only (local development); everything else must be `https`. */
function checkUrl(what: string, url: string): string {
  let u: URL
  try {
    u = new URL(url)
  } catch {
    throw new OidcError('discovery', `The provider's ${what} is not a valid URL: ${url}`)
  }
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(u.hostname)
  if (u.protocol !== 'https:' && !(u.protocol === 'http:' && loopback)) {
    throw new OidcError('discovery', `The provider's ${what} must use https: ${url}`)
  }
  return u.toString()
}

/** The provider's endpoints: from the server's configuration when it names them, else from discovery. */
export async function discover(
  p: Provider,
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
): Promise<Endpoints> {
  if (p.authorization_endpoint && p.token_endpoint) {
    return {
      authorization: checkUrl('authorization endpoint', p.authorization_endpoint),
      token: checkUrl('token endpoint', p.token_endpoint),
      endSession: p.end_session_endpoint
        ? checkUrl('logout endpoint', p.end_session_endpoint)
        : undefined,
    }
  }
  const url = `${trimSlash(p.issuer)}/.well-known/openid-configuration`
  let doc: Record<string, unknown>
  try {
    const res = await fetchFn(url, { credentials: 'omit', headers: { Accept: 'application/json' } })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    doc = (await res.json()) as Record<string, unknown>
  } catch (e) {
    throw new OidcError(
      'discovery',
      `Could not read the sign-in provider's configuration from ${url} (${e instanceof Error ? e.message : 'failed'}). ` +
        'The provider must be reachable from this browser and allow requests from this origin (CORS).',
    )
  }
  if (typeof doc.issuer !== 'string' || !sameIssuer(doc.issuer, p.issuer)) {
    throw new OidcError(
      'discovery',
      `The provider at ${p.issuer} describes itself as a different issuer.`,
    )
  }
  const authorization = p.authorization_endpoint ?? doc.authorization_endpoint
  const token = p.token_endpoint ?? doc.token_endpoint
  if (typeof authorization !== 'string' || typeof token !== 'string') {
    throw new OidcError(
      'discovery',
      "The provider's configuration has no authorization or token endpoint.",
    )
  }
  const endSession = p.end_session_endpoint ?? doc.end_session_endpoint
  return {
    authorization: checkUrl('authorization endpoint', authorization),
    token: checkUrl('token endpoint', token),
    endSession:
      typeof endSession === 'string' ? checkUrl('logout endpoint', endSession) : undefined,
  }
}

/** The URL to send the browser to. */
export function authorizationUrl(
  p: Provider,
  ep: Endpoints,
  o: { state: string; nonce: string; challenge: string; redirect: string; silent?: boolean },
): string {
  const u = new URL(ep.authorization)
  const q = u.searchParams
  q.set('response_type', 'code')
  q.set('client_id', p.client_id)
  q.set('redirect_uri', o.redirect)
  q.set('scope', p.scopes.join(' '))
  q.set('state', o.state)
  q.set('nonce', o.nonce)
  q.set('code_challenge', o.challenge)
  q.set('code_challenge_method', 'S256')
  if (o.silent) q.set('prompt', 'none')
  return u.toString()
}

/** Starts a flow: everything to remember, and where to go. */
export async function beginFlow(
  p: Provider,
  next: string,
  origin: string,
  o: { silent?: boolean; fetchFn?: typeof fetch; now?: () => number } = {},
): Promise<{ flow: Flow; url: string }> {
  const endpoints = await discover(p, o.fetchFn)
  const verifier = randomToken(32)
  const flow: Flow = {
    state: randomToken(24),
    nonce: randomToken(24),
    verifier,
    issuer: p.issuer,
    clientId: p.client_id,
    endpoints,
    next,
    silent: !!o.silent,
    startedAt: (o.now ?? Date.now)(),
  }
  const url = authorizationUrl(p, endpoints, {
    state: flow.state,
    nonce: flow.nonce,
    challenge: await codeChallenge(verifier),
    redirect: redirectUri(origin),
    silent: o.silent,
  })
  return { flow, url }
}

export interface Callback {
  code?: string
  state?: string
  error?: string
  errorDescription?: string
}

export function parseCallback(search: string | URLSearchParams): Callback {
  const q = typeof search === 'string' ? new URLSearchParams(search) : search
  const get = (k: string) => q.get(k) ?? undefined
  return {
    code: get('code'),
    state: get('state'),
    error: get('error'),
    errorDescription: get('error_description'),
  }
}

/** Decodes the claims of a JWT without verifying anything (the server verifies). `null` if it is not one. */
export function jwtClaims(token: string): Record<string, unknown> | null {
  const part = token.split('.')[1]
  if (token.split('.').length !== 3 || !part) return null
  try {
    const b64 = part
      .replace(/-/g, '+')
      .replace(/_/g, '/')
      .padEnd(Math.ceil(part.length / 4) * 4, '=')
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))
    const claims: unknown = JSON.parse(new TextDecoder().decode(bytes))
    return claims && typeof claims === 'object' && !Array.isArray(claims)
      ? (claims as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

/** The checks of the callback that need no network. Throws the error to show. */
export function checkCallback(cb: Callback, flow: Flow | null): string {
  if (!flow) {
    throw new OidcError(
      'state',
      'There is no sign-in in progress for this browser tab (it was started elsewhere, or already completed). Start again.',
    )
  }
  if (cb.error) {
    if (
      cb.error === 'login_required' ||
      cb.error === 'interaction_required' ||
      cb.error === 'consent_required'
    ) {
      throw new OidcError('login_required', 'Your single sign-on session has ended. Sign in again.')
    }
    const why = cb.errorDescription ? `: ${cb.errorDescription}` : ''
    throw new OidcError(
      cb.error === 'access_denied' ? 'denied' : 'token',
      cb.error === 'access_denied'
        ? `The provider refused the sign-in${why || '.'}`
        : `The provider reported an error (${cb.error})${why}`,
    )
  }
  if (!cb.state || !constantTimeEqual(cb.state, flow.state)) {
    throw new OidcError(
      'state',
      'The sign-in response did not match the request that started it, so it was ignored (this can be a forged link). Start again.',
    )
  }
  if (!cb.code) throw new OidcError('token', 'The provider did not return an authorization code.')
  return cb.code
}

function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false
  let diff = 0
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i)
  return diff === 0
}

interface TokenResponse {
  access_token?: string
  id_token?: string
  refresh_token?: string
  expires_in?: number
  error?: string
  error_description?: string
}

async function postToken(
  endpoint: string,
  body: Record<string, string>,
  fetchFn: typeof fetch,
): Promise<TokenResponse> {
  let res: Response
  try {
    res = await fetchFn(endpoint, {
      method: 'POST',
      credentials: 'omit',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
      body: new URLSearchParams(body).toString(),
    })
  } catch {
    throw new OidcError(
      'token',
      'Could not reach the provider to finish signing in. It must allow requests from this origin (CORS) on its token endpoint.',
    )
  }
  let json: TokenResponse = {}
  try {
    json = (await res.json()) as TokenResponse
  } catch {
    /* not JSON */
  }
  if (!res.ok || json.error) {
    const detail = json.error_description ?? json.error ?? `HTTP ${res.status}`
    throw new OidcError('token', `The provider did not give a token: ${detail}`)
  }
  return json
}

/** Checks the claims of an ID token against what was asked for. `nonce` is only checked when given. */
export function checkIdToken(
  idToken: string,
  expect: { issuer: string; clientId: string; nonce?: string },
  nowMs: number,
): void {
  const claims = jwtClaims(idToken)
  const bad = (m: string) => new OidcError('id_token', `The ID token was refused: ${m}`)
  if (!claims) throw bad('it is not a JWT.')
  if (typeof claims.iss !== 'string' || !sameIssuer(claims.iss, expect.issuer)) {
    throw bad('it was issued by a different provider.')
  }
  const aud = claims.aud
  const audiences = Array.isArray(aud) ? aud : [aud]
  if (!audiences.includes(expect.clientId)) throw bad('it was issued for a different application.')
  if (typeof claims.exp === 'number' && claims.exp * 1000 <= nowMs - 30_000) {
    throw bad('it has already expired.')
  }
  if (expect.nonce !== undefined && claims.nonce !== expect.nonce) {
    throw new OidcError(
      'nonce',
      'The ID token does not belong to this sign-in (its nonce differs), so it was ignored. Start again.',
    )
  }
}

/** Picks the bearer and when it expires. */
export function tokensFrom(res: TokenResponse, p: Pick<Provider, 'token'>, nowMs: number): Tokens {
  const bearer = p.token === 'id_token' ? res.id_token : res.access_token
  if (!bearer) {
    throw new OidcError(
      'token',
      `The provider's answer has no ${p.token === 'id_token' ? 'ID token (is the "openid" scope allowed for this client?)' : 'access token'}.`,
    )
  }
  const exp = jwtClaims(bearer)?.exp
  const expiresAtMs =
    typeof exp === 'number'
      ? exp * 1000
      : typeof res.expires_in === 'number'
        ? nowMs + res.expires_in * 1000
        : 0
  return { bearer, idToken: res.id_token, refreshToken: res.refresh_token, expiresAtMs }
}

/** Trades the authorization code (and the PKCE verifier) for tokens. */
export async function exchangeCode(
  p: Provider,
  flow: Flow,
  code: string,
  origin: string,
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
  nowMs: number = Date.now(),
): Promise<Tokens> {
  const res = await postToken(
    flow.endpoints.token,
    {
      grant_type: 'authorization_code',
      code,
      redirect_uri: redirectUri(origin),
      client_id: p.client_id,
      code_verifier: flow.verifier,
    },
    fetchFn,
  )
  if (res.id_token) {
    checkIdToken(
      res.id_token,
      { issuer: p.issuer, clientId: p.client_id, nonce: flow.nonce },
      nowMs,
    )
  } else if (p.token === 'id_token') {
    throw new OidcError(
      'token',
      'The provider did not return an ID token (is the "openid" scope allowed for this client?).',
    )
  }
  return tokensFrom(res, p, nowMs)
}

/** Uses a refresh token to get a new bearer, without the user. A refused refresh means: sign in again. */
export async function refreshTokens(
  p: Provider,
  endpoints: Endpoints,
  refreshToken: string,
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
  nowMs: number = Date.now(),
): Promise<Tokens> {
  const res = await postToken(
    endpoints.token,
    { grant_type: 'refresh_token', refresh_token: refreshToken, client_id: p.client_id },
    fetchFn,
  )
  if (res.id_token) checkIdToken(res.id_token, { issuer: p.issuer, clientId: p.client_id }, nowMs)
  const tokens = tokensFrom(res, p, nowMs)
  // Providers may rotate the refresh token or keep the old one.
  return { ...tokens, refreshToken: res.refresh_token ?? refreshToken }
}

/** Where to send the browser to end the provider's session as well, when it has such an endpoint. */
export function logoutUrl(
  p: Provider,
  endpoints: Endpoints | undefined,
  idToken: string | undefined,
  origin: string,
): string | null {
  if (!endpoints?.endSession) return null
  const u = new URL(endpoints.endSession)
  if (idToken) u.searchParams.set('id_token_hint', idToken)
  u.searchParams.set('client_id', p.client_id)
  u.searchParams.set('post_logout_redirect_uri', `${origin}/signin`)
  return u.toString()
}
