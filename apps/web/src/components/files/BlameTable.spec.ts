import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { ApiError } from '@/api/errors'
import type { BlameLine, BlameResponse } from '@/api/types'

enableAutoUnmount(afterEach)

const blame = vi.fn()
vi.mock('@/api/instance', () => ({ api: { history: { blame: (...a: unknown[]) => blame(...a) } } }))

import BlameTable from './BlameTable.vue'

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:p(.*)*', component: { template: '<i/>' } }],
})

const line = (n: number, changeset: string, extra: Partial<BlameLine> = {}): BlameLine => ({
  line: n,
  original_line: n,
  changeset,
  author: 'user:alice',
  summary: `Change ${changeset}`,
  text: `text ${n}`,
  timestamp_ms: Date.now() - 60_000,
  path: 'src/a.ts',
  boundary: false,
  ...extra,
})
const reply = (lines: BlameLine[], extra: Partial<BlameResponse> = {}): BlameResponse => ({
  lines,
  path: 'src/a.ts',
  rev: 'c0ffee',
  truncated: false,
  visited: 3,
  ...extra,
})
const mountIt = () =>
  mount(BlameTable, {
    props: { repo: 'acme/webapp', rev: 'main', path: 'src/a.ts' },
    global: { plugins: [router] },
  })

beforeEach(() => {
  blame.mockReset()
})

describe('BlameTable', () => {
  it.each(['repo', 'rev', 'path'] as const)(
    'cancels obsolete blame when %s changes',
    async (key) => {
      const pending: { signal: AbortSignal; resolve: (value: BlameResponse) => void }[] = []
      blame.mockImplementation(
        (_repo, _query, options) =>
          new Promise<BlameResponse>((resolve) => {
            pending.push({ signal: options?.signal, resolve })
          }),
      )
      const w = mountIt()
      expect(pending[0]!.signal).toBeInstanceOf(AbortSignal)
      await w.setProps({ [key]: 'changed' })
      expect(pending[0]!.signal.aborted).toBe(true)
      expect(pending[1]!.signal.aborted).toBe(false)
      pending[1]!.resolve(reply([line(1, 'current', { text: 'Current file' })]))
      await flushPromises()
      pending[0]!.resolve(reply([line(1, 'obsolete', { text: 'Obsolete file' })]))
      await flushPromises()
      expect(w.text()).toContain('Current file')
      expect(w.text()).not.toContain('Obsolete file')
    },
  )

  it('cancels pending blame when leaving the file', () => {
    let signal: AbortSignal | undefined
    blame.mockImplementation((_repo, _query, options) => {
      signal = options?.signal
      return new Promise(() => {})
    })
    const w = mountIt()
    expect(signal).toBeInstanceOf(AbortSignal)
    w.unmount()
    expect(signal!.aborted).toBe(true)
  })

  it('asks for the path at the revision and shows one author block per run of lines', async () => {
    blame.mockResolvedValue(
      reply([
        line(1, 'aaaaaaaa11'),
        line(2, 'aaaaaaaa11'),
        line(3, 'bbbbbbbb22', { author: 'agent:refactor-bot#run_1', summary: 'Extract the store' }),
      ]),
    )
    const w = mountIt()
    await flushPromises()
    expect(blame).toHaveBeenCalledWith(
      'acme/webapp',
      { path: 'src/a.ts', rev: 'main' },
      { signal: expect.any(AbortSignal) },
    )
    expect(w.findAll('tbody tr')).toHaveLength(3)
    expect(w.findAll('td.who')).toHaveLength(2)
    expect(w.find('td.who').attributes('rowspan')).toBe('2')
    expect(w.text()).toContain('Extract the store')
    expect(w.text()).toContain('3 changesets visited')
    expect(w.find('table').attributes('aria-label')).toBe('Line-by-line authorship of src/a.ts')
  })

  it('links each block to its changeset in the history', async () => {
    blame.mockResolvedValue(reply([line(1, 'aaaaaaaa11')]))
    const w = mountIt()
    await flushPromises()
    const a = w.find('td.who a')
    expect(a.text()).toBe('aaaaaaaa')
    expect(a.attributes('href')).toContain('/repos/acme/webapp/history')
    expect(a.attributes('href')).toContain('cs=aaaaaaaa11')
  })

  it('says which lines are only "older than shown", and when the walk was cut short', async () => {
    blame.mockResolvedValue(reply([line(1, 'old', { boundary: true })], { truncated: true }))
    const w = mountIt()
    await flushPromises()
    expect(w.text()).toContain('older than shown')
    expect(w.text()).toContain('The walk stopped early')
  })

  it('shows a renamed file under the name it had then', async () => {
    blame.mockResolvedValue(reply([line(1, 'aaaaaaaa11', { path: 'src/old-name.ts' })]))
    const w = mountIt()
    await flushPromises()
    expect(w.text()).toContain('as src/old-name.ts')
  })

  it('renders hostile text and summaries as text', async () => {
    const evil = '<img src=x onerror="window.__pwned=1">'
    blame.mockResolvedValue(reply([line(1, 'aaaaaaaa11', { text: evil, summary: evil })]))
    const w = mountIt()
    await flushPromises()
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('td.code').text()).toBe(evil)
    expect(w.find('.summary').text()).toBe(evil)
  })

  it('shows a long file in pieces', async () => {
    blame.mockResolvedValue(
      reply(Array.from({ length: 1500 }, (_, i) => line(i + 1, i < 700 ? 'a' : 'b'))),
    )
    const w = mountIt()
    await flushPromises()
    expect(w.findAll('tbody tr')).toHaveLength(1000)
    expect(w.text()).toContain('1,000 of 1,500 shown')
    await w.find('.more button').trigger('click')
    expect(w.findAll('tbody tr')).toHaveLength(1500)
    expect(w.find('.more').exists()).toBe(false)
  })

  it('explains a server without blame instead of showing an error', async () => {
    blame.mockImplementation(() =>
      Promise.reject(new ApiError({ status: 501, code: 'unimplemented', message: 'no' })),
    )
    const w = mountIt()
    await flushPromises()
    expect(w.text()).toContain('Blame is not available on this server')
  })

  it('shows other failures with a retry', async () => {
    blame.mockImplementation(() =>
      Promise.reject(new ApiError({ status: 500, code: 'internal', message: 'boom' })),
    )
    const w = mountIt()
    await flushPromises()
    expect(w.text()).toContain('boom')
    expect(w.find('button').exists()).toBe(true)
  })

  it('handles an empty file', async () => {
    blame.mockResolvedValue(reply([]))
    const w = mountIt()
    await flushPromises()
    expect(w.text()).toContain('This file is empty.')
  })
})
