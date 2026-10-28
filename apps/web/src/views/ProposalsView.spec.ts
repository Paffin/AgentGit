import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import LoadMore from '@/ui/LoadMore.vue'
import ProposalsView from './ProposalsView.vue'

enableAutoUnmount(afterEach)
afterEach(() => vi.useRealTimers())
const list = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { proposals: { list } } }))
vi.mock('@/stores/auth', () => ({ useAuth: () => ({ can: () => false }) }))
const page = (title: string, nextPageToken = '') => ({
  items: [
    { id: title, title, author: 'user:alice', stream: 'main', state: 'open', updated_at_ms: 1 },
  ],
  nextPageToken,
})
beforeEach(() => {
  vi.useFakeTimers()
  list.mockReset().mockResolvedValue(page('First request', 'next-page'))
})
async function show(path = '/repos/acme/demo/proposals') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos/:owner/:name/proposals', component: ProposalsView },
      { path: '/repos/:owner/:name/proposals/:id', component: { template: '<i />' } },
    ],
  })
  await router.push(path)
  const w = mount(
    { components: { RouterView }, template: '<RouterView />' },
    { global: { plugins: [router] } },
  )
  await flushPromises()
  return { w, router }
}

describe('change request pagination', () => {
  it('retains loaded links after a next-page failure and retries the same cursor', async () => {
    list
      .mockResolvedValueOnce(page('First request', 'next-page'))
      .mockRejectedValueOnce(new Error('Page unavailable'))
      .mockResolvedValueOnce(page('Second request'))
    const { w } = await show()
    w.findComponent(LoadMore).vm.$emit('more')
    await flushPromises()
    expect(w.findAll('a.t').map((link) => link.text())).toEqual(['First request'])
    expect(w.text()).toContain('Page unavailable')
    w.findComponent(LoadMore).vm.$emit('more')
    await flushPromises()
    expect(list.mock.calls[1]![1].page_token).toBe('next-page')
    expect(list.mock.calls[2]![1].page_token).toBe('next-page')
    expect(w.findAll('a.t').map((link) => link.text())).toEqual(['First request', 'Second request'])
    expect(w.text()).not.toContain('Page unavailable')
  })

  it('does not start a duplicate page request while one is pending', async () => {
    let finish!: (value: ReturnType<typeof page>) => void
    list.mockResolvedValueOnce(page('First request', 'next-page')).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w } = await show()
    w.findComponent(LoadMore).vm.$emit('more')
    w.findComponent(LoadMore).vm.$emit('more')
    await flushPromises()
    expect(list).toHaveBeenCalledTimes(2)
    finish(page('Second request'))
    await flushPromises()
    expect(w.findAll('a.t').map((link) => link.text())).toEqual(['First request', 'Second request'])
  })

  it.each(['repository', 'state', 'author'])(
    'invalidates a pending page immediately when %s changes',
    async (context) => {
      let finish!: (value: ReturnType<typeof page>) => void
      list
        .mockResolvedValueOnce(page('First request', 'next-page'))
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finish = resolve
            }),
        )
        .mockResolvedValueOnce(page('New context'))
      const { w, router } = await show()
      w.findComponent(LoadMore).vm.$emit('more')
      await flushPromises()
      if (context === 'repository') await router.push('/repos/other/repository/proposals')
      else if (context === 'state') await w.get('input[value="closed"]').setValue(true)
      else await w.get('input[placeholder="e.g. agent:refactor-bot"]').setValue('user:bob')
      expect(w.findAll('a.t')).toHaveLength(0)
      finish(page('Obsolete page'))
      await flushPromises()
      expect(w.findAll('a.t')).toHaveLength(0)
      await vi.advanceTimersByTimeAsync(200)
      await flushPromises()
      expect(w.findAll('a.t').map((link) => link.text())).toEqual(['New context'])
      expect(list.mock.lastCall![1].page_token).toBeUndefined()
      if (context === 'repository') expect(list.mock.lastCall![0]).toBe('other/repository')
      if (context === 'state') expect(list.mock.lastCall![1].state).toBe('closed')
      if (context === 'author') expect(list.mock.lastCall![1].author).toBe('user:bob')
    },
  )

  it('retries first-page failures without a pagination cursor', async () => {
    list
      .mockRejectedValueOnce(new Error('Initial load failed'))
      .mockResolvedValueOnce(page('Recovered'))
    const { w } = await show()
    expect(w.text()).toContain('Initial load failed')
    await w
      .findAll('button')
      .find((button) => button.text() === 'Try again')!
      .trigger('click')
    await flushPromises()
    expect(list.mock.lastCall![1].page_token).toBeUndefined()
    expect(w.findAll('a.t').map((link) => link.text())).toEqual(['Recovered'])
  })
})

describe('change request filter navigation', () => {
  it('keeps State, Author and search when opening a request and returning with Back', async () => {
    const { w, router } = await show()
    await w.get('input[value="closed"]').setValue(true)
    await w.get('input[placeholder="e.g. agent:refactor-bot"]').setValue('user:bob')
    await w.get('input[type=search]').setValue('First')
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({
      state: 'closed',
      author: 'user:bob',
      q: 'First',
    })
    await w.get('a.t').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toContain('/proposals/First%20request')
    router.back()
    await flushPromises()
    expect((w.get('input[value="closed"]').element as HTMLInputElement).checked).toBe(true)
    expect(
      w.get<HTMLInputElement>('input[placeholder="e.g. agent:refactor-bot"]').element.value,
    ).toBe('user:bob')
    expect(w.get<HTMLInputElement>('input[type=search]').element.value).toBe('First')
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: 'closed', author: 'user:bob' }),
    )
  })

  it('reads bookmarked filters and responds to Back/Forward query changes', async () => {
    const { w, router } = await show(
      '/repos/acme/demo/proposals?state=integrated&author=user:bob&q=First',
    )
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: 'integrated', author: 'user:bob' }),
    )
    await router.push({ query: { state: 'all', author: 'user:carol', q: 'First' } })
    await vi.advanceTimersByTimeAsync(200)
    expect(list).toHaveBeenCalledTimes(2)
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: undefined, author: 'user:carol' }),
    )
    router.back()
    await flushPromises()
    await vi.advanceTimersByTimeAsync(200)
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: 'integrated', author: 'user:bob' }),
    )
    router.forward()
    await flushPromises()
    await vi.advanceTimersByTimeAsync(200)
    expect((w.get('input[value="all"]').element as HTMLInputElement).checked).toBe(true)
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: undefined, author: 'user:carol' }),
    )
  })

  it('defaults repeated or empty filters and does not reload for a local text search', async () => {
    const { w, router } = await show(
      '/repos/acme/demo/proposals?state=closed&state=all&author=a&author=b&q=x&q=y',
    )
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: 'open', author: undefined }),
    )
    expect(w.get<HTMLInputElement>('input[type=search]').element.value).toBe('')
    await router.push({ query: { q: 'First', state: '', author: '' } })
    await vi.advanceTimersByTimeAsync(200)
    expect(w.get<HTMLInputElement>('input[type=search]').element.value).toBe('First')
    expect(list).toHaveBeenCalledTimes(1)
  })
  it('keeps simultaneous edits together and coalesces their server request', async () => {
    const { w, router } = await show()
    await Promise.all([
      w.get('input[value="closed"]').setValue(true),
      w.get('input[placeholder="e.g. agent:refactor-bot"]').setValue('user:bob'),
      w.get('input[type=search]').setValue('First'),
    ])
    await flushPromises()
    await vi.advanceTimersByTimeAsync(200)
    expect(router.currentRoute.value.query).toEqual({
      state: 'closed',
      author: 'user:bob',
      q: 'First',
    })
    expect(list).toHaveBeenCalledTimes(2)
    expect(list.mock.lastCall![1]).toEqual(
      expect.objectContaining({ state: 'closed', author: 'user:bob' }),
    )
  })
})
