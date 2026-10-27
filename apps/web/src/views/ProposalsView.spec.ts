import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
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
async function show() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos/:owner/:name/proposals', component: ProposalsView },
      { path: '/repos/:owner/:name/proposals/:id', component: { template: '<i />' } },
    ],
  })
  await router.push('/repos/acme/demo/proposals')
  const w = mount(ProposalsView, { global: { plugins: [router] } })
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
