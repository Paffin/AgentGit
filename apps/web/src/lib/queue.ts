import type { QueueEntry } from '@/api/types'

export type QueueTone = 'ok' | 'warn' | 'bad' | 'info' | 'neutral'

/** The states the server gives a queue entry (`QueueEntry.state` in the OpenAPI document). */
export const QUEUE_TONE: Record<string, QueueTone> = {
  queued: 'info',
  done: 'ok',
  blocked: 'warn',
  rejected: 'bad',
  failed: 'bad',
}

export interface QueueRunSummary {
  tone: 'ok' | 'warn'
  title: string
  detail?: string
}

/**
 * What a `queue.run` answered, in words. The engine looks at every queued entry: one that is blocked or rejected is
 * marked and the entries behind it still run, so `stopped_at` only names the first of them, and "n processed" is
 * every entry, not those before a stop.
 */
export function summarizeQueueRun(processed: QueueEntry[]): QueueRunSummary {
  if (processed.length === 0) return { tone: 'ok', title: 'The queue was empty' }
  const count = (state: string) => processed.filter((e) => e.state === state).length
  const parts: string[] = []
  const done = count('done')
  if (done) parts.push(`${done} integrated`)
  for (const state of ['blocked', 'rejected', 'failed']) {
    const n = count(state)
    if (n) parts.push(`${n} ${state}`)
  }
  const other = processed.filter(
    (e) => !['done', 'blocked', 'rejected', 'failed'].includes(e.state),
  ).length
  if (other > 0) parts.push(`${other} still waiting`)
  const allDone = done === processed.length
  return {
    tone: allDone ? 'ok' : 'warn',
    title: parts.join(', '),
    detail: allDone ? undefined : 'Open the entries marked blocked, rejected or failed to see why.',
  }
}
