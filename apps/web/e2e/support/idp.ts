// A small OpenID Connect provider for the end-to-end tests: just enough of a real one to run the browser's
// authorization-code flow with PKCE against a real ag-server, including the ways it goes wrong. It signs EdDSA
// tokens with a throw-away key and publishes the key at /jwks, which is where the server fetches it.
//
// It plays the provider honestly: the redirect URI must be the registered one, the PKCE verifier must match the
// challenge, a code works once, the `nonce` it was given comes back in the ID token, the session lives in a
// cookie of its own. The login page is a plain form: the test types a user name and picks what the provider
// should do ("scenario"). Everything it does is recorded and can be read back at /__test/events?user=NAME.
import { createHash, generateKeyPairSync, randomBytes, sign as edSign } from 'node:crypto'
import type { KeyObject } from 'node:crypto'
import { createServer } from 'node:http'
import type { IncomingMessage, ServerResponse } from 'node:http'

export const IDP_CLIENT_ID = 'agentgit-web'

/** What the provider does when the user signs in. */
export const SCENARIOS = [
  'normal',
  'expired-token',
  'wrong-state',
  'bad-nonce',
  'denied',
  'bad-signature',
  'short-renewable',
  'short-no-refresh',
] as const
export type Scenario = (typeof SCENARIOS)[number]

export interface IdpEvent {
  kind: 'authorize' | 'login' | 'token' | 'refresh' | 'logout' | 'error'
  user?: string
  detail: Record<string, unknown>
}

export interface RunningIdp {
  issuer: string
  clientId: string
  stop(): Promise<void>
}

interface CodeRecord {
  user: string
  scenario: Scenario
  nonce: string
  challenge: string
  redirectUri: string
  scope: string
}

const b64u = (b: Buffer | string) => Buffer.from(b).toString('base64url')

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer()
    s.once('error', reject)
    s.listen(0, '127.0.0.1', () => {
      const addr = s.address()
      const port = typeof addr === 'object' && addr ? addr.port : 0
      s.close(() => resolve(port))
    })
  })
}

function html(strings: TemplateStringsArray, ...values: string[]): string {
  const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`)
  return strings.reduce(
    (out, s, i) => out + s + (i < values.length ? esc(values[i] ?? '') : ''),
    '',
  )
}

export async function startIdp(): Promise<RunningIdp> {
  const port = await freePort()
  const issuer = `http://127.0.0.1:${port}`
  const kid = 'e2e-key-1'
  const { privateKey, publicKey } = generateKeyPairSync('ed25519')
  const other = generateKeyPairSync('ed25519') // for the "bad signature" scenario
  const jwk = { ...publicKey.export({ format: 'jwk' }), kid, alg: 'EdDSA', use: 'sig' }

  const codes = new Map<string, CodeRecord>()
  const refreshTokens = new Map<string, { user: string; scenario: Scenario }>()
  const events: IdpEvent[] = []
  const note = (e: IdpEvent) => events.push(e)

  const jwt = (claims: Record<string, unknown>, key: KeyObject): string => {
    const head = b64u(JSON.stringify({ alg: 'EdDSA', typ: 'JWT', kid }))
    const body = b64u(JSON.stringify(claims))
    const sig = edSign(null, Buffer.from(`${head}.${body}`), key)
    return `${head}.${body}.${b64u(sig)}`
  }

  function idTokenFor(user: string, scenario: Scenario, nonce: string | undefined): string {
    const now = Math.floor(Date.now() / 1000)
    const ttl =
      scenario === 'expired-token'
        ? -3600
        : scenario === 'short-renewable'
          ? 5
          : scenario === 'short-no-refresh'
            ? 6
            : 600
    const groups = user === 'dana' ? ['staff', 'platform'] : ['staff']
    return jwt(
      {
        iss: issuer,
        aud: IDP_CLIENT_ID,
        sub: `sub-${user}`,
        preferred_username: user,
        groups,
        iat: now + Math.min(ttl, 0) - 10,
        exp: now + ttl,
        nonce: scenario === 'bad-nonce' ? 'not-the-nonce-you-sent' : nonce,
        jti: randomBytes(8).toString('hex'),
      },
      scenario === 'bad-signature' ? other.privateKey : privateKey,
    )
  }

  function tokenResponse(user: string, scenario: Scenario, nonce: string | undefined) {
    const id_token = idTokenFor(user, scenario, nonce)
    const body: Record<string, unknown> = {
      token_type: 'Bearer',
      id_token,
      access_token: `opaque-${randomBytes(12).toString('hex')}`,
      expires_in: scenario === 'short-renewable' ? 5 : scenario === 'short-no-refresh' ? 6 : 600,
      scope: 'openid profile',
    }
    if (scenario !== 'short-no-refresh') {
      const refresh = `refresh-${randomBytes(12).toString('hex')}`
      refreshTokens.set(refresh, {
        user,
        scenario: scenario === 'short-renewable' ? 'normal' : scenario,
      })
      body.refresh_token = refresh
    }
    return body
  }

  const cors = (res: ServerResponse) => {
    res.setHeader('Access-Control-Allow-Origin', '*')
    res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Accept')
    res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
  }
  const json = (res: ServerResponse, status: number, body: unknown) => {
    cors(res)
    res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' })
    res.end(JSON.stringify(body))
  }
  const redirect = (
    res: ServerResponse,
    location: string,
    headers: Record<string, string> = {},
  ) => {
    res.writeHead(302, { Location: location, 'Cache-Control': 'no-store', ...headers })
    res.end()
  }
  const readBody = (req: IncomingMessage): Promise<string> =>
    new Promise((resolve) => {
      let data = ''
      req.on('data', (c: Buffer) => (data += c.toString()))
      req.on('end', () => resolve(data))
    })
  const cookieUser = (req: IncomingMessage): { user: string; scenario: Scenario } | null => {
    const m = /(?:^|;\s*)idp_session=([^;]+)/.exec(req.headers.cookie ?? '')
    if (!m?.[1]) return null
    const [user, scenario] = decodeURIComponent(m[1]).split('|')
    return user ? { user, scenario: (scenario as Scenario) || 'normal' } : null
  }

  const registeredRedirect = /^http:\/\/127\.0\.0\.1:\d+\/auth\/callback$/

  async function handle(req: IncomingMessage, res: ServerResponse) {
    const url = new URL(req.url ?? '/', issuer)
    const q = url.searchParams
    if (req.method === 'OPTIONS') {
      cors(res)
      res.writeHead(204)
      res.end()
      return
    }

    if (url.pathname === '/favicon.ico') {
      res.writeHead(204)
      res.end()
      return
    }
    if (url.pathname === '/.well-known/openid-configuration') {
      return json(res, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        end_session_endpoint: `${issuer}/logout`,
        jwks_uri: `${issuer}/jwks`,
        response_types_supported: ['code'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
        code_challenge_methods_supported: ['S256'],
        id_token_signing_alg_values_supported: ['EdDSA'],
        subject_types_supported: ['public'],
      })
    }
    if (url.pathname === '/jwks') return json(res, 200, { keys: [jwk] })

    if (url.pathname === '/authorize') {
      const fail = (m: string) => {
        note({ kind: 'error', detail: { path: '/authorize', message: m } })
        res.writeHead(400, { 'Content-Type': 'text/plain' })
        res.end(`invalid request: ${m}`)
      }
      const redirectUri = q.get('redirect_uri') ?? ''
      if (q.get('client_id') !== IDP_CLIENT_ID) return fail('unknown client_id')
      if (!registeredRedirect.test(redirectUri)) return fail('redirect_uri is not registered')
      if (q.get('response_type') !== 'code') return fail('response_type must be code')
      if (q.get('code_challenge_method') !== 'S256' || !q.get('code_challenge')) {
        return fail('PKCE with S256 is required')
      }
      if (!(q.get('scope') ?? '').split(' ').includes('openid'))
        return fail('scope must include openid')
      const state = q.get('state') ?? ''
      const back = (params: Record<string, string>) => {
        const u = new URL(redirectUri)
        for (const [k, v] of Object.entries(params)) u.searchParams.set(k, v)
        if (state && !('state' in params)) u.searchParams.set('state', state)
        return u.toString()
      }
      const session = cookieUser(req)
      note({
        kind: 'authorize',
        user: session?.user,
        detail: {
          prompt: q.get('prompt'),
          hasSession: !!session,
          challengeMethod: q.get('code_challenge_method'),
          scope: q.get('scope'),
          stateLength: state.length,
          nonceLength: (q.get('nonce') ?? '').length,
        },
      })
      if (q.get('prompt') === 'none') {
        if (!session) return redirect(res, back({ error: 'login_required' }))
        const code = randomBytes(16).toString('hex')
        codes.set(code, {
          user: session.user,
          scenario: session.scenario,
          nonce: q.get('nonce') ?? '',
          challenge: q.get('code_challenge') ?? '',
          redirectUri,
          scope: q.get('scope') ?? '',
        })
        return redirect(res, back({ code }))
      }
      res.writeHead(200, {
        'Content-Type': 'text/html; charset=utf-8',
        'Cache-Control': 'no-store',
      })
      const hidden = [...q.entries()]
        .map(([k, v]) => html`<input type="hidden" name="q_${k}" value="${v}" />`)
        .join('')
      const options = SCENARIOS.map((s) => html`<option value="${s}">${s}</option>`).join('')
      res.end(
        `<!doctype html><meta charset="utf-8"><title>Fake identity provider</title>
<h1>Fake identity provider</h1>
<form method="post" action="/login">${hidden}
<p><label>Username <input name="username" value="dana" autocomplete="off"></label></p>
<p><label>What should happen <select name="scenario">${options}</select></label></p>
<p><button type="submit">Sign in</button></p></form>`,
      )
      return
    }

    if (url.pathname === '/login' && req.method === 'POST') {
      const form = new URLSearchParams(await readBody(req))
      const user = (form.get('username') ?? '').trim() || 'dana'
      const scenario = (form.get('scenario') as Scenario) || 'normal'
      const orig = (k: string) => form.get(`q_${k}`) ?? ''
      const redirectUri = orig('redirect_uri')
      if (!registeredRedirect.test(redirectUri)) {
        res.writeHead(400)
        res.end('bad redirect_uri')
        return
      }
      note({ kind: 'login', user, detail: { scenario } })
      const u = new URL(redirectUri)
      const state = orig('state')
      const cookie = `idp_session=${encodeURIComponent(`${user}|${scenario}`)}; Path=/; HttpOnly; SameSite=Lax`
      if (scenario === 'denied') {
        u.searchParams.set('error', 'access_denied')
        u.searchParams.set('error_description', 'The user said no')
        u.searchParams.set('state', state)
        return redirect(res, u.toString(), { 'Set-Cookie': cookie })
      }
      const code = randomBytes(16).toString('hex')
      codes.set(code, {
        user,
        scenario,
        nonce: orig('nonce'),
        challenge: orig('code_challenge'),
        redirectUri,
        scope: orig('scope'),
      })
      u.searchParams.set('code', code)
      u.searchParams.set('state', scenario === 'wrong-state' ? 'forged-state-value' : state)
      return redirect(res, u.toString(), { 'Set-Cookie': cookie })
    }

    if (url.pathname === '/token' && req.method === 'POST') {
      const form = new URLSearchParams(await readBody(req))
      const bad = (error: string, description: string) => {
        note({ kind: 'error', detail: { path: '/token', error, description } })
        json(res, 400, { error, error_description: description })
      }
      if (form.get('client_id') !== IDP_CLIENT_ID) return bad('invalid_client', 'unknown client')
      if (form.has('client_secret'))
        return bad('invalid_request', 'a public client sends no secret')
      const grant = form.get('grant_type')
      if (grant === 'authorization_code') {
        const code = form.get('code') ?? ''
        const rec = codes.get(code)
        codes.delete(code) // single use
        if (!rec) return bad('invalid_grant', 'unknown or used code')
        if (form.get('redirect_uri') !== rec.redirectUri)
          return bad('invalid_grant', 'redirect_uri differs')
        const verifier = form.get('code_verifier') ?? ''
        const expected = createHash('sha256').update(verifier).digest('base64url')
        if (!verifier || expected !== rec.challenge)
          return bad('invalid_grant', 'PKCE verification failed')
        note({
          kind: 'token',
          user: rec.user,
          detail: { grant, pkce: 'verified', verifierLength: verifier.length },
        })
        return json(res, 200, tokenResponse(rec.user, rec.scenario, rec.nonce))
      }
      if (grant === 'refresh_token') {
        const rt = form.get('refresh_token') ?? ''
        const rec = refreshTokens.get(rt)
        if (!rec) return bad('invalid_grant', 'unknown refresh token')
        refreshTokens.delete(rt) // rotation: each refresh token is used once
        note({ kind: 'refresh', user: rec.user, detail: { grant } })
        return json(res, 200, tokenResponse(rec.user, rec.scenario, undefined))
      }
      return bad('unsupported_grant_type', String(grant))
    }

    if (url.pathname === '/logout') {
      const session = cookieUser(req)
      note({
        kind: 'logout',
        user: session?.user,
        detail: {
          idTokenHint: !!q.get('id_token_hint'),
          clientId: q.get('client_id'),
          hadSession: !!session,
        },
      })
      const target = q.get('post_logout_redirect_uri')
      const clear = 'idp_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax'
      if (target && registeredRedirect.test(target.replace(/\/signin$/, '/auth/callback'))) {
        return redirect(res, target, { 'Set-Cookie': clear })
      }
      res.writeHead(200, { 'Content-Type': 'text/plain', 'Set-Cookie': clear })
      res.end('signed out')
      return
    }

    if (url.pathname === '/__test/events') {
      const user = q.get('user')
      return json(res, 200, user ? events.filter((e) => e.user === user) : events)
    }
    res.writeHead(404)
    res.end('not found')
  }

  const server = createServer((req, res) => {
    handle(req, res).catch((e: unknown) => {
      note({ kind: 'error', detail: { message: e instanceof Error ? e.message : String(e) } })
      res.writeHead(500)
      res.end('error')
    })
  })
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, '127.0.0.1', resolve)
  })
  return {
    issuer,
    clientId: IDP_CLIENT_ID,
    stop: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections()
        server.close(() => resolve())
      }),
  }
}
