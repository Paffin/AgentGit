import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import HistoryView from './HistoryView.vue'

const log = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { history: { log } } }))
vi.mock('@/stores/board', () => ({
  useBoard: () => ({
    repo: { default_stream: 'main' },
    load: { repo: { status: 'ready' } },
    streams: [],
  }),
}))

function page(message: string) {
  return {
    items: [{ id: message, parents: [], message, author: 'user:alice', timestamp_ms: 1 }],
    nextPageToken: '',
  }
}
async function show(path = '/repos/acme/demo/history?rev=main') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/repos/:owner/:name/history', component: HistoryView }],
  })
  await router.push(path)
  const w = mount(HistoryView, {
    global: { plugins: [router], stubs: { ChangesetDetail: true } },
  })
  await flushPromises()
  return { w, router }
}
beforeEach(() => {
  log
    .mockReset()
    .mockImplementation(async (_repo, query) => page(`${query.rev}:${query.path ?? 'all'}`))
})

describe('history navigation', () => {
  it('follows revision and path filters through Back and Forward', async () => {
    const { w, router } = await show('/repos/acme/demo/history?rev=main&path=src/first.ts')
    await router.push('/repos/acme/demo/history?rev=release&path=src/second.ts')
    await flushPromises()
    expect(w.get('input').element.value).toBe('release')
    expect(w.text()).toContain('release:src/second.ts')
    expect(w.text()).not.toContain('main:src/first.ts')
    router.back()
    await flushPromises()
    expect(w.get('input').element.value).toBe('main')
    expect(w.text()).toContain('main:src/first.ts')
    router.forward()
    await flushPromises()
    expect(w.text()).toContain('release:src/second.ts')
    expect(log).toHaveBeenCalledTimes(4)
    w.unmount()
  })

  it('does not discard unapplied typing or reload when only selection changes', async () => {
    const { w, router } = await show()
    await w.get('input').setValue('work in progress')
    await router.push({ query: { rev: 'main', cs: 'chosen' } })
    await flushPromises()
    expect(w.get('input').element.value).toBe('work in progress')
    expect(log).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('loads the URL filters in the new repository and ignores a late old response', async () => {
    const { w, router } = await show()
    let finish!: (value: ReturnType<typeof page>) => void
    log.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await router.push('/repos/acme/demo/history?rev=slow')
    await flushPromises()
    await router.push('/repos/other/repository/history?rev=release&path=lib/file.ts')
    await flushPromises()
    expect(log).toHaveBeenLastCalledWith(
      'other/repository',
      expect.objectContaining({
        rev: 'release',
        path: 'lib/file.ts',
      }),
    )
    finish(page('stale response'))
    await flushPromises()
    expect(w.text()).toContain('release:lib/file.ts')
    expect(w.text()).not.toContain('stale response')
    w.unmount()
  })

  it('uses the default branch when query filters are removed or repeated', async () => {
    const { w, router } = await show('/repos/acme/demo/history?rev=release&path=src/file.ts')
    await router.push('/repos/acme/demo/history?rev=one&rev=two&path=a&path=b')
    await flushPromises()
    expect(w.get('input').element.value).toBe('')
    expect(log).toHaveBeenLastCalledWith(
      'acme/demo',
      expect.objectContaining({
        rev: 'main',
        path: undefined,
      }),
    )
    expect(w.text()).toContain('main:all')
    await router.push('/repos/acme/demo/history')
    await flushPromises()
    expect(log).toHaveBeenCalledTimes(2)
    w.unmount()
  })

  it('clears and restores a path filter without discarding an unapplied revision', async () => {
    const { w, router } = await show('/repos/acme/demo/history?rev=main&path=src/file.ts')
    await w.get('input').setValue('not applied')
    await w.get('button.clear').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.query.path).toBeUndefined()
    expect(w.get('input').element.value).toBe('not applied')
    expect(w.text()).toContain('main:all')
    router.back()
    await flushPromises()
    expect(w.text()).toContain('main:src/file.ts')
    expect(w.get('input').element.value).toBe('not applied')
    expect(log).toHaveBeenCalledTimes(3)
    w.unmount()
  })

  it('refreshes the same applied revision once without adding history', async () => {
    const { w, router } = await show()
    const url = router.currentRoute.value.fullPath
    await w.get('input').setValue(' main ')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(w.get('input').element.value).toBe('main')
    expect(router.currentRoute.value.fullPath).toBe(url)
    expect(log).toHaveBeenCalledTimes(2)
    w.unmount()
  })

  it('applies an edited revision exactly once and supports Back to the previous filter', async () => {
    const { w, router } = await show()
    await w.get('input').setValue(' release ')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(router.currentRoute.value.query.rev).toBe('release')
    expect(log).toHaveBeenCalledTimes(2)
    router.back()
    await flushPromises()
    expect(w.get('input').element.value).toBe('main')
    expect(w.text()).toContain('main:all')
    w.unmount()
  })
})
