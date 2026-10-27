/**
 * The sign-in link of `ag-server dev`: `http://127.0.0.1:8080/repos/demo/welcome#token=agt_…`.
 *
 * The credential is in the URL *fragment*. A browser never sends a fragment to the server, so it is in no access
 * log and no `Referer`; this module removes it from the address bar (and from the history entry of the page, with
 * `history.replaceState`) before the app does anything else, and hands it to the sign-in once.
 *
 * What this does not do: the browser's own history database may keep the address as it was typed or clicked, and
 * whoever can see the terminal where the link was printed can see it. That is why the credential `ag-server dev`
 * issues is short-lived and replaced at every start; see docs/getting-started.md.
 */

/** The fragment parameter that carries the credential. */
export const LOGIN_PARAM = 'token'

/**
 * Reads the credential out of the fragment of `location` and removes it from the address bar. Returns it, or null
 * when the fragment carries none. Other fragment parameters stay.
 */
export function takeLoginFragment(
  location: Pick<Location, 'hash' | 'pathname' | 'search'>,
  history: Pick<History, 'replaceState' | 'state'>,
): string | null {
  const raw = location.hash.startsWith('#') ? location.hash.slice(1) : location.hash
  if (!raw) return null
  // Not URLSearchParams: it would read a "+" in a credential as a space.
  const parts = raw.split('&')
  const mine = parts.filter((p) => p === LOGIN_PARAM || p.startsWith(`${LOGIN_PARAM}=`))
  if (mine.length === 0) return null
  const rest = parts.filter((p) => !mine.includes(p)).join('&')
  const decode = (value: string): string => {
    try {
      return decodeURIComponent(value)
    } catch {
      return ''
    }
  }
  const token = decode((mine[0] ?? '').slice(LOGIN_PARAM.length + 1))
  // First, before anything can fail or wait: the credential must not stay in the address bar.
  history.replaceState(
    history.state,
    '',
    location.pathname + location.search + (rest ? `#${rest}` : ''),
  )
  return token.length > 0 ? token : null
}
