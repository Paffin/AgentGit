import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Proposal } from '@/api/types'

const api = vi.hoisted(() => ({
  history: { mergeBase: vi.fn(), changeset: vi.fn() },
  streams: { reflog: vi.fn() },
}))
vi.mock('@/api/instance', () => ({ api }))

import { diffBase } from './useProposalData'

const proposal = (over: Partial<Proposal> = {}): Proposal => ({
  id: 'prop_1',
  stream: 'main',
  head: 'h1',
  title: 't',
  description: '',
  author: 'user:a',
  state: 'open',
  task_id: '',
  created_at_ms: 0,
  updated_at_ms: 0,
  integrated_as: '',
  ...over,
})

describe('diffBase', () => {
  beforeEach(() => vi.resetAllMocks())

  it('is the merge base of an open proposal, and the empty tree only when the server says there is none', async () => {
    api.history.mergeBase.mockResolvedValueOnce(['b1'])
    expect(await diffBase('r', proposal())).toBe('b1')
    api.history.mergeBase.mockResolvedValueOnce([])
    expect(await diffBase('r', proposal())).toBe('')
  })

  it('fails when the merge base cannot be read, instead of showing the whole tree as added', async () => {
    api.history.mergeBase.mockRejectedValueOnce(new Error('network blip'))
    await expect(diffBase('r', proposal())).rejects.toThrow('network blip')
  })

  it('uses the reflog move for an integrated proposal, then the landed changeset, and fails when both are unreadable', async () => {
    const landed = proposal({ state: 'integrated', integrated_as: 'c9' })
    api.streams.reflog.mockResolvedValueOnce([{ new_head: 'c9', old_head: 'c8' }])
    expect(await diffBase('r', landed)).toBe('c8')

    api.streams.reflog.mockResolvedValueOnce([{ new_head: 'x', old_head: 'y' }])
    api.history.changeset.mockResolvedValueOnce({ parents: ['p1'] })
    expect(await diffBase('r', landed)).toBe('p1')

    api.streams.reflog.mockRejectedValueOnce(new Error('403'))
    await expect(diffBase('r', landed)).rejects.toThrow('403')
    expect(api.history.changeset).toHaveBeenCalledTimes(1)
  })
})
