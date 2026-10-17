// The one API instance of the running app. The token lives here (in memory) and in sessionStorage via the
// auth store; this module never touches storage, never logs, and never puts the token in a URL.
import { ApiClient } from './client'
import { createApi } from './endpoints'
import type { ApiError } from './errors'

let currentToken: string | null = null
const unauthenticatedHandlers = new Set<(e: ApiError) => void>()

export function setApiToken(token: string | null): void {
  currentToken = token
}

export function hasApiToken(): boolean {
  return currentToken !== null
}

/** Called when any request (except a sign-in attempt) is answered with 401. Returns an unsubscribe function. */
export function onUnauthenticated(fn: (e: ApiError) => void): () => void {
  unauthenticatedHandlers.add(fn)
  return () => unauthenticatedHandlers.delete(fn)
}

export const client = new ApiClient({
  baseUrl: '',
  getToken: () => currentToken,
  onUnauthenticated: (e) => {
    for (const fn of unauthenticatedHandlers) fn(e)
  },
})

export const api = createApi(client)
