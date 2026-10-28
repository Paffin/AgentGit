import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Changeset, DiffResponse } from '@/api/types'
import HistoryView from './HistoryView.vue'

enableAutoUnmount(afterEach)

const mocks = vi.hoisted(() => ({ changeset: vi.fn(), diff: vi.fn(), log: vi.fn() }))
vi.mock('@/api/instance', () => ({
  api: { history: { changeset: mocks.changeset, log: mocks.log }, tree: { diff: mocks.diff } },
}))
vi.mock('@/stores/board', () => ({
  useBoard: () => ({
    repo: { default_stream: 'main' },
    load: { repo: { status: 'ready' } },
    streams: [],
  }),
}))

const changeset = (id: string): Changeset => ({
  id,
  author: 'user:alice',
  generation: 1,
  message: `Changeset ${id}`,
  namespace: '',
  ops: [],
  parents: [],
  published: true,
  timestamp_ms: 1,
})
const diff = (path: string): DiffResponse => ({
  files: [
    {
      path,
      status: 'added',
      binary: false,
      additions: 1,
      deletions: 0,
      patch: '@@ -0,0 +1 @@\n+new line\n',
    },
  ],
  truncated: false,
})
async function show(path = '/repos/acme/demo/history?cs=first') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/repos/:owner/:name/history', component: HistoryView }],
  })
  await router.push(path)
  const w = mount(HistoryView, { global: { plugins: [router] } })
  await flushPromises()
  return { w, router }
}
beforeEach(() => {
  mocks.changeset.mockReset().mockImplementation(async (_repo, id) => changeset(id))
  mocks.diff.mockReset().mockImplementation(async (_repo, query) => diff(`${query.to}.ts`))
  mocks.log
    .mockReset()
    .mockResolvedValue({ items: [changeset('first'), changeset('second')], nextPageToken: '' })
})

describe('changeset detail navigation', () => {
  it.each([
    '/repos/acme/demo/history?cs=second',
    '/repos/other/repository/history?cs=second',
    '/repos/acme/demo/history',
  ])('cancels pending metadata when navigating to %s', async (destination) => {
    let signal: AbortSignal | undefined
    let finish!: (value: Changeset) => void
    mocks.changeset.mockImplementationOnce((_repo, _id, _ops, options) => {
      signal = options?.signal
      return new Promise((resolve) => {
        finish = resolve
      })
    })
    const { w, router } = await show()
    expect(signal).toBeInstanceOf(AbortSignal)
    await router.push(destination)
    await flushPromises()
    expect(signal!.aborted).toBe(true)
    finish(changeset('obsolete'))
    await flushPromises()
    expect(w.text()).not.toContain('Changeset obsolete')
    if (destination.includes('second')) expect(w.text()).toContain('Changeset second')
  })

  it.each([
    '/repos/acme/demo/history?cs=second',
    '/repos/other/repository/history?cs=second',
    '/repos/acme/demo/history',
  ])('cancels pending diff when navigating to %s', async (destination) => {
    let signal: AbortSignal | undefined
    let finish!: (value: DiffResponse) => void
    mocks.diff.mockImplementationOnce((_repo, _query, options) => {
      signal = options?.signal
      return new Promise((resolve) => {
        finish = resolve
      })
    })
    const { w, router } = await show()
    await w.get('button.link').trigger('click')
    expect(signal).toBeInstanceOf(AbortSignal)
    await router.push(destination)
    await flushPromises()
    expect(signal!.aborted).toBe(true)
    finish(diff('obsolete.ts'))
    await flushPromises()
    expect(w.text()).not.toContain('obsolete.ts')
    if (destination.includes('second')) {
      await w.get('button.link').trigger('click')
      await flushPromises()
      expect(w.text()).toContain('second.ts')
    }
  })

  it('does not label the previous diff as the newly selected changeset', async () => {
    const { w, router } = await show()
    await w.get('button.link').trigger('click')
    await flushPromises()
    expect(w.get('[data-testid="changeset-detail"]').text()).toContain('first.ts')
    await router.push({ query: { cs: 'second' } })
    await flushPromises()
    const detail = w.get('[data-testid="changeset-detail"]')
    expect(detail.text()).toContain('Changeset second')
    expect(detail.text()).not.toContain('first.ts')
    expect(detail.text()).toContain('Show the diff')
    expect(mocks.diff).toHaveBeenCalledTimes(1)
    await detail.get('button.link').trigger('click')
    await flushPromises()
    expect(detail.text()).toContain('second.ts')
    w.unmount()
  })

  it('ignores a diff arriving after its changeset is no longer selected', async () => {
    let finish!: (value: DiffResponse) => void
    mocks.diff.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await show()
    await w.get('button.link').trigger('click')
    await router.push({ query: { cs: 'second' } })
    await flushPromises()
    await w.get('button.link').trigger('click')
    await flushPromises()
    finish(diff('late-first.ts'))
    await flushPromises()
    expect(w.get('[data-testid="changeset-detail"]').text()).toContain('second.ts')
    expect(w.text()).not.toContain('late-first.ts')
    w.unmount()
  })

  it('starts fresh after a failed diff and when the repository changes', async () => {
    mocks.diff.mockRejectedValueOnce(new Error('first diff failed'))
    const { w, router } = await show()
    await w.get('button.link').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('first diff failed')
    await router.push('/repos/other/repository/history?cs=first')
    await flushPromises()
    expect(w.text()).not.toContain('first diff failed')
    expect(w.text()).toContain('Show the diff')
    expect(mocks.changeset).toHaveBeenLastCalledWith('other/repository', 'first', true, {
      signal: expect.any(AbortSignal),
    })
    w.unmount()
  })

  it('removes the detail when selection is cleared, including a pending diff', async () => {
    let finish!: (value: DiffResponse) => void
    mocks.diff.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await show()
    await w.get('button.link').trigger('click')
    await router.push({ query: {} })
    await flushPromises()
    finish(diff('late-first.ts'))
    await flushPromises()
    expect(w.find('[data-testid="changeset-detail"]').exists()).toBe(false)
    expect(w.text()).not.toContain('late-first.ts')
    w.unmount()
  })
})

describe('parent changeset links', () => {
  beforeEach(() => {
    mocks.changeset.mockImplementation(async (_repo, id) => ({
      ...changeset(id),
      parents: ['parent'],
    }))
  })

  it('follows one real link while preserving the revision and path filters', async () => {
    const { w, router } = await show('/repos/acme/demo/history?rev=topic&path=src/file.ts&cs=first')
    const parent = w.get('.par a')
    const url = new URL(parent.attributes('href') ?? '', 'http://localhost')
    expect(url.pathname).toBe('/repos/acme/demo/history')
    expect(url.searchParams.get('rev')).toBe('topic')
    expect(url.searchParams.get('path')).toBe('src/file.ts')
    expect(url.searchParams.get('cs')).toBe('parent')
    let navigations = 0
    const stop = router.afterEach(() => {
      navigations++
    })
    await parent.trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({
      rev: 'topic',
      path: 'src/file.ts',
      cs: 'parent',
    })
    expect(navigations).toBe(1)
    stop()
    w.unmount()
  })

  it.each([
    ['Control', { ctrlKey: true }],
    ['Command', { metaKey: true }],
    ['Shift', { shiftKey: true }],
    ['Alt', { altKey: true }],
    ['middle button', { button: 1 }],
  ])('does not change the current selection for %s click', async (_name, event) => {
    const { w, router } = await show('/repos/acme/demo/history?rev=topic&path=src/file.ts&cs=first')
    await w.get('.par a').trigger('click', event)
    await flushPromises()
    expect(router.currentRoute.value.query.cs).toBe('first')
    expect(w.get('[data-testid="changeset-detail"]').text()).toContain('Changeset first')
    w.unmount()
  })
})
