import { describe, expect, it } from 'vitest'
import type { QueueEntry } from '@/api/types'
import { QUEUE_TONE, summarizeQueueRun } from './queue'

const entry = (state: string, id = 1): QueueEntry => ({
  id,
  stream: 'main',
  proposal: `prop_${id}`,
  state,
  outcome: '',
  enqueued_by: 'user:a',
  enqueued_at_ms: 0,
  processed_at_ms: 0,
})

describe('queue', () => {
  it('uses the states the server defines', () => {
    expect(Object.keys(QUEUE_TONE).sort()).toEqual([
      'blocked',
      'done',
      'failed',
      'queued',
      'rejected',
    ])
    expect(QUEUE_TONE.rejected).toBe('bad')
  })

  it('does not claim the run stopped: blocked entries are skipped and the rest are counted', () => {
    const s = summarizeQueueRun([
      entry('done', 1),
      entry('blocked', 2),
      entry('done', 3),
      entry('rejected', 4),
    ])
    expect(s.title).toBe('2 integrated, 1 blocked, 1 rejected')
    expect(s.tone).toBe('warn')
    expect(s.title).not.toMatch(/stopped|before/)
  })

  it('is plain success when everything was integrated, and says so for an empty queue', () => {
    expect(summarizeQueueRun([entry('done'), entry('done', 2)])).toEqual({
      tone: 'ok',
      title: '2 integrated',
      detail: undefined,
    })
    expect(summarizeQueueRun([])).toEqual({ tone: 'ok', title: 'The queue was empty' })
  })
})
