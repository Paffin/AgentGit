import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, onUnauthenticated, setApiToken } from '@/api/instance'
import { ApiError } from '@/api/errors'
import { serverNow } from '@/api/clock'
import type { AuthConfig, Capabilities, Principal, Role } from '@/api/types'
import {
  OidcError,
  beginFlow,
  checkCallback,
  exchangeCode,
  logoutUrl,
  parseCallback,
  refreshTokens,
  sameIssuer,
} from '@/lib/oidc'
import type { Endpoints, Flow, Provider, Tokens } from '@/lib/oidc'
import { readStore, removeStore, writeStore } from '@/lib/storage'
import { mayDo, parseRoleGrants, rolesForRepo, satisfies } from '@/lib/roles'
import type { Action } from '@/lib/roles'

/** A pasted token lives in sessionStorage only: gone when the tab closes, never in localStorage or a URL. */
export const TOKEN_KEY = 'agentgit.token'
/**
 * Redirect sign-in: what must survive the trip to the provider and back (PKCE verifier, state, nonce). Removed the
 * moment the callback is handled. The token obtained with it is never stored anywhere but memory.
 */
export const FLOW_KEY = 'agentgit.oidc.flow'
/** Says "this tab signed in through this provider": no secret, only whom to ask again after a reload. */
export const SSO_KEY = 'agentgit.sso'
const FLOW_MAX_AGE_MS = 10 * 60_000
/** Renew this long before the token expires (when the provider gave a refresh token). */
const RENEW_EARLY_MS = 60_000

/** Accepts what people paste: strips whitespace and a leading "Bearer ". Returns null if it cannot be a token. */
export function cleanToken(raw: string): string | null {
  let t = raw.trim().replace(/^bearer\s+/i, '')
  t = t.replace(/\s+/g, '')
  // Visible ASCII only: anything else cannot travel in an HTTP header.
  if (t.length < 8 || !/^[\x21-\x7e]+$/.test(t)) return null
  return t
}

export type AuthStatus = 'anonymous' | 'checking' | 'ready'

export const useAuth = defineStore('auth', () => {
  const token = ref<string | null>(null)
  const principal = ref<Principal | null>(null)
  const capabilities = ref<Capabilities | null>(null)
  const status = ref<AuthStatus>('anonymous')
  /** Why the last session ended, shown on the sign-in page. */
  const endedBecause = ref<'expired' | 'rejected' | 'signed-out' | 'sso' | ''>('')
  /** The message of a failed redirect sign-in (`endedBecause === 'sso'`). */
  const ssoMessage = ref('')
  const expiryWarned = ref(false)
  /** How this session was started: a pasted token, or a redirect sign-in. */
  const method = ref<'token' | 'oidc' | null>(null)
  const config = ref<AuthConfig | null>(null)
  // Redirect sign-in keeps its credentials in memory only (see FLOW_KEY): a reload asks the provider again.
  let sso: {
    provider: Provider
    endpoints: Endpoints
    idToken?: string
    refreshToken?: string
  } | null = null
  let expiryTimer: ReturnType<typeof setTimeout> | undefined
  let warnTimer: ReturnType<typeof setTimeout> | undefined
  let renewTimer: ReturnType<typeof setTimeout> | undefined

  const isAuthenticated = computed(() => status.value === 'ready' && principal.value !== null)
  const identity = computed(() => principal.value?.identity ?? '')
  const grants = computed(() => parseRoleGrants(principal.value?.roles ?? []))
  const expiresAt = computed(() => principal.value?.expires_at_ms ?? 0)
  const isAdminAnywhere = computed(() => grants.value.some((g) => g.role === 'admin'))

  function rolesFor(repo: string): Set<Role> {
    return rolesForRepo(grants.value, repo)
  }
  function hasRole(repo: string, role: Role): boolean {
    return satisfies(rolesFor(repo), role)
  }
  function can(repo: string, action: Action): boolean {
    return mayDo(rolesFor(repo), action)
  }

  function clearTimers() {
    clearTimeout(expiryTimer)
    clearTimeout(warnTimer)
    clearTimeout(renewTimer)
    expiryTimer = warnTimer = renewTimer = undefined
  }

  function scheduleExpiry() {
    clearTimers()
    expiryWarned.value = false
    const at = expiresAt.value
    if (!at) return
    const left = at - serverNow()
    // setTimeout caps at ~24.8 days; longer-lived credentials simply do not need a timer.
    if (left <= 0 || left > 2 ** 31 - 1) return
    expiryTimer = setTimeout(() => end('expired'), left)
    // A session that renews itself has nothing to warn about; if renewing is refused, the warning is raised then.
    if (sso?.refreshToken) {
      renewTimer = setTimeout(() => void renew(), Math.max(1_000, left - RENEW_EARLY_MS))
      return
    }
    const warnIn = left - 120_000
    if (warnIn > 0 && warnIn < 2 ** 31 - 1) {
      warnTimer = setTimeout(() => {
        expiryWarned.value = true
      }, warnIn)
    } else if (left > 0 && left <= 120_000) {
      expiryWarned.value = true
    }
  }

  /** Silent renewal with the provider's refresh token. If the provider refuses, the session simply runs out. */
  async function renew() {
    const current = sso
    if (!current?.refreshToken || status.value !== 'ready') return
    try {
      const tokens = await refreshTokens(current.provider, current.endpoints, current.refreshToken)
      if (sso !== current || status.value !== 'ready') return
      current.refreshToken = tokens.refreshToken
      current.idToken = tokens.idToken ?? current.idToken
      token.value = tokens.bearer
      setApiToken(tokens.bearer)
      principal.value = await api.whoami()
      scheduleExpiry()
    } catch {
      if (sso !== current) return
      current.refreshToken = undefined
      expiryWarned.value = true
    }
  }

  function end(reason: 'expired' | 'rejected' | 'signed-out' | 'sso') {
    clearTimers()
    token.value = null
    principal.value = null
    status.value = 'anonymous'
    endedBecause.value = reason
    method.value = null
    sso = null
    setApiToken(null)
    removeStore('session', TOKEN_KEY)
    removeStore('session', SSO_KEY)
  }

  async function loadCapabilities() {
    try {
      capabilities.value = await api.capabilities()
    } catch {
      capabilities.value = null
    }
  }

  /** Checks a pasted token against the server and, if good, starts the session. Throws ApiError otherwise. */
  async function signIn(raw: string): Promise<Principal> {
    const t = cleanToken(raw)
    if (!t) {
      throw new ApiError({
        status: 0,
        code: 'invalid_argument',
        message: 'That does not look like a token. Paste the whole value, with no spaces.',
      })
    }
    status.value = 'checking'
    try {
      const who = await api.whoami(t)
      token.value = t
      principal.value = who
      method.value = 'token'
      sso = null
      setApiToken(t)
      writeStore('session', TOKEN_KEY, t)
      removeStore('session', SSO_KEY)
      status.value = 'ready'
      endedBecause.value = ''
      scheduleExpiry()
      void loadCapabilities()
      return who
    } catch (e) {
      status.value = 'anonymous'
      throw e
    }
  }

  // ---- redirect sign-in (OpenID Connect, authorization code + PKCE) -------------------------------

  /** What the server offers for signing in. Without an answer (old server, offline) it is the pasted token. */
  async function loadAuthConfig(): Promise<AuthConfig> {
    if (config.value) return config.value
    try {
      config.value = await api.authConfig()
    } catch {
      return { mode: 'token' }
    }
    return config.value
  }

  function providerFor(cfg: AuthConfig, issuer: string, clientId: string): Provider | null {
    const found = (cfg.providers ?? []).find(
      (p) => sameIssuer(p.issuer, issuer) && p.client_id === clientId,
    )
    return found ?? null
  }

  /**
   * Sends the browser to the identity provider. `silent` asks it to show nothing (`prompt=none`): used to resume
   * after a reload. Resolves once the navigation has been started.
   */
  async function startSso(provider: Provider, next: string, silent = false): Promise<void> {
    const { flow, url } = await beginFlow(provider, next, window.location.origin, { silent })
    if (!writeStore('session', FLOW_KEY, JSON.stringify(flow))) {
      throw new OidcError(
        'unsupported',
        'This browser blocks session storage, which redirect sign-in needs to survive the round trip. Paste a token instead.',
      )
    }
    window.location.assign(url)
  }

  /** Signs in with provider number `index` of the configuration. */
  async function signInWithSso(index: number, next: string): Promise<void> {
    const cfg = await loadAuthConfig()
    const provider = cfg.providers?.[index]
    if (!provider) {
      throw new OidcError('config', 'That sign-in provider is not configured on this server.')
    }
    try {
      await startSso(provider, next)
    } catch (e) {
      throw asOidcError(e)
    }
  }

  function readFlow(): Flow | null {
    const raw = readStore('session', FLOW_KEY)
    removeStore('session', FLOW_KEY) // one use only, whatever happens next
    if (!raw) return null
    try {
      const flow = JSON.parse(raw) as Flow
      if (typeof flow.state !== 'string' || typeof flow.verifier !== 'string') return null
      if (Date.now() - flow.startedAt > FLOW_MAX_AGE_MS) return null
      return flow
    } catch {
      return null
    }
  }

  function asOidcError(e: unknown): OidcError | ApiError {
    if (e instanceof OidcError || e instanceof ApiError) return e
    return new OidcError('token', e instanceof Error ? e.message : 'Sign-in failed.')
  }

  /**
   * Finishes a redirect sign-in at `/auth/callback`: checks `state`, trades the code for tokens, checks the ID
   * token's nonce, issuer and audience, and asks the server who this is. Resolves to where to go next. On any
   * failure the session stays anonymous and `ssoMessage` says why (the sign-in page shows it).
   */
  async function completeSso(search: string): Promise<string> {
    const flow = readFlow()
    status.value = 'checking'
    try {
      const code = checkCallback(parseCallback(search), flow)
      const cfg = await loadAuthConfig()
      const provider = flow ? providerFor(cfg, flow.issuer, flow.clientId) : null
      if (!flow || !provider) {
        throw new OidcError(
          'config',
          'The sign-in provider is no longer configured on this server.',
        )
      }
      const tokens: Tokens = await exchangeCode(provider, flow, code, window.location.origin)
      let who: Principal
      try {
        who = await api.whoami(tokens.bearer)
      } catch (e) {
        if (e instanceof ApiError && e.isUnauthenticated) {
          throw new OidcError(
            'token',
            `This server did not accept the ${provider.token === 'id_token' ? 'ID' : 'access'} token that ${provider.name} issued. ` +
              'Ask an administrator to check that the server trusts this provider and that the token’s audience matches.',
          )
        }
        throw e
      }
      token.value = tokens.bearer
      principal.value = who
      method.value = 'oidc'
      sso = {
        provider,
        endpoints: flow.endpoints,
        idToken: tokens.idToken,
        refreshToken: tokens.refreshToken,
      }
      setApiToken(tokens.bearer)
      removeStore('session', TOKEN_KEY)
      writeStore(
        'session',
        SSO_KEY,
        JSON.stringify({ issuer: provider.issuer, clientId: provider.client_id }),
      )
      status.value = 'ready'
      endedBecause.value = ''
      ssoMessage.value = ''
      scheduleExpiry()
      void loadCapabilities()
      return flow.next
    } catch (e) {
      const err = asOidcError(e)
      end('sso')
      ssoMessage.value =
        err instanceof ApiError
          ? err.isNetwork
            ? 'Could not reach the server to finish signing in.'
            : err.message
          : err.message
      throw err
    }
  }

  /** After a reload: if this tab signed in through a provider, ask it again (it answers at once if its own session lives). */
  async function resumeSso(next: string): Promise<boolean> {
    const raw = readStore('session', SSO_KEY)
    if (!raw) return false
    try {
      const marker = JSON.parse(raw) as { issuer: string; clientId: string }
      const cfg = await loadAuthConfig()
      const provider = providerFor(cfg, marker.issuer, marker.clientId)
      if (!provider) {
        removeStore('session', SSO_KEY)
        return false
      }
      await startSso(provider, next, true)
      return true
    } catch (e) {
      removeStore('session', SSO_KEY)
      ssoMessage.value = asOidcError(e).message
      endedBecause.value = 'sso'
      return false
    }
  }

  /** On page load: resume the session from sessionStorage if the token is still good. */
  async function restore(next = '/'): Promise<void> {
    if (status.value === 'ready' || status.value === 'checking') return
    const saved = readStore('session', TOKEN_KEY)
    if (!saved) {
      status.value = 'anonymous'
      // This tab signed in through a provider before the reload: ask it again, silently. The browser leaves the
      // page; hold the navigation until it does.
      if (await resumeSso(next)) await new Promise<never>(() => undefined)
      return
    }
    status.value = 'checking'
    try {
      const who = await api.whoami(saved)
      token.value = saved
      principal.value = who
      setApiToken(saved)
      status.value = 'ready'
      scheduleExpiry()
      void loadCapabilities()
    } catch (e) {
      if (e instanceof ApiError && e.isNetwork) {
        // The server is unreachable, not the token wrong: keep the saved token (so a retry resumes the
        // session) and let the app show "offline" instead of a sign-in form.
        status.value = 'anonymous'
        throw e
      }
      end('rejected')
    }
  }

  async function refreshWhoami() {
    if (!token.value) return
    principal.value = await api.whoami()
    scheduleExpiry()
  }

  /** Signs in again through the same provider (the session is about to end and cannot renew itself). */
  async function reauthenticate(next: string): Promise<void> {
    if (!sso) return
    try {
      await startSso(sso.provider, next)
    } catch (e) {
      throw asOidcError(e)
    }
  }

  /**
   * Ends the session. For a redirect sign-in whose provider offers it, returns the URL that also ends the
   * provider's own session (the caller navigates there); otherwise `null`.
   */
  function signOut(): string | null {
    const out = sso
      ? logoutUrl(sso.provider, sso.endpoints, sso.idToken, window.location.origin)
      : null
    end('signed-out')
    return out
  }

  // Any other request answered with 401 means the session is over.
  onUnauthenticated(() => {
    if (status.value === 'ready') end('expired')
  })

  return {
    token,
    principal,
    capabilities,
    status,
    endedBecause,
    ssoMessage,
    method,
    config,
    expiryWarned,
    isAuthenticated,
    identity,
    grants,
    expiresAt,
    isAdminAnywhere,
    rolesFor,
    hasRole,
    can,
    signIn,
    loadAuthConfig,
    signInWithSso,
    completeSso,
    reauthenticate,
    restore,
    refreshWhoami,
    signOut,
    loadCapabilities,
  }
})
