import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, onUnauthenticated, setApiToken } from '@/api/instance'
import { ApiError } from '@/api/errors'
import { serverNow } from '@/api/clock'
import type { Capabilities, Principal, Role } from '@/api/types'
import { readStore, removeStore, writeStore } from '@/lib/storage'
import { mayDo, parseRoleGrants, rolesForRepo, satisfies } from '@/lib/roles'
import type { Action } from '@/lib/roles'

/** The token lives in sessionStorage only: gone when the tab closes, never in localStorage or a URL. */
export const TOKEN_KEY = 'agentgit.token'

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
  const endedBecause = ref<'expired' | 'rejected' | 'signed-out' | ''>('')
  const expiryWarned = ref(false)
  let expiryTimer: ReturnType<typeof setTimeout> | undefined
  let warnTimer: ReturnType<typeof setTimeout> | undefined

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
    expiryTimer = warnTimer = undefined
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
    const warnIn = left - 120_000
    if (warnIn > 0 && warnIn < 2 ** 31 - 1) {
      warnTimer = setTimeout(() => {
        expiryWarned.value = true
      }, warnIn)
    } else if (left > 0 && left <= 120_000) {
      expiryWarned.value = true
    }
  }

  function end(reason: 'expired' | 'rejected' | 'signed-out') {
    clearTimers()
    token.value = null
    principal.value = null
    status.value = 'anonymous'
    endedBecause.value = reason
    setApiToken(null)
    removeStore('session', TOKEN_KEY)
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
      setApiToken(t)
      writeStore('session', TOKEN_KEY, t)
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

  /** On page load: resume the session from sessionStorage if the token is still good. */
  async function restore(): Promise<void> {
    if (status.value === 'ready' || status.value === 'checking') return
    const saved = readStore('session', TOKEN_KEY)
    if (!saved) {
      status.value = 'anonymous'
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

  function signOut() {
    end('signed-out')
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
    restore,
    refreshWhoami,
    signOut,
    loadCapabilities,
  }
})
