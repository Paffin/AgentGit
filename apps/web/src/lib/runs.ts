import type { Run } from '@/api/types'

export type Liveness = 'fresh' | 'stale' | 'finished'

export interface RunHealth {
  liveness: Liveness
  /** Milliseconds since the last heartbeat (or since the start if it never heartbeated). */
  ageMs: number
}

export const DEFAULT_STALE_AFTER_MS = 120_000

/**
 * A run is *stale* when it claims to be running but has been silent for longer than `staleAfterMs`.
 * Agents heartbeat at their own pace, so the threshold is a per-viewer preference.
 */
export function runHealth(run: Run, now: number, staleAfterMs = DEFAULT_STALE_AFTER_MS): RunHealth {
  if (run.status !== 'running') {
    return { liveness: run.status === 'stale' ? 'stale' : 'finished', ageMs: 0 }
  }
  const last = run.last_heartbeat_ms || run.started_at_ms
  const ageMs = Math.max(0, now - last)
  return { liveness: ageMs >= staleAfterMs ? 'stale' : 'fresh', ageMs }
}

export function isActive(run: Run): boolean {
  return run.status === 'running' || run.status === 'stale'
}
