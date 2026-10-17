// Server time, as far as the browser can tell. Lease countdowns and heartbeat ages compare server
// timestamps with "now", so a skewed laptop clock would make every lease look expired or eternal.
// The only signal we have for free is the HTTP `Date` header (1 s resolution), so small differences are
// treated as noise and only a real skew is corrected.

const NOISE_MS = 1500

let offsetMs = 0

/** Feed a `Date` header value (server time) observed at `clientNow`. */
export function observeServerDate(serverDateMs: number, clientNow: number = Date.now()): void {
  if (!Number.isFinite(serverDateMs)) return
  // The header is truncated to whole seconds: the true instant lies in [D, D + 1000).
  const estimate = serverDateMs + 500 - clientNow
  offsetMs = Math.abs(estimate) < NOISE_MS ? 0 : estimate
}

/** Milliseconds to add to the local clock to get server time. */
export function serverOffset(): number {
  return offsetMs
}

export function serverNow(clientNow: number = Date.now()): number {
  return clientNow + offsetMs
}

/** For tests. */
export function resetServerClock(): void {
  offsetMs = 0
}
