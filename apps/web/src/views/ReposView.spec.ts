import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ReposView from './ReposView.vue'
const list = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({
  api: { repos: { list } },
  onUnauthenticated: () => () => {},
  setApiToken: () => {},
}))
function repo(name: string) {
  return {
    name,
    description: '',
    default_stream: 'main',
    stream_count: 1,
    changeset_count: 1,
    created_at_ms: 1,
    counts_error: '',
  }
}
async function show() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos', component: ReposView },
      { path: '/repos/:owner/:name', name: 'board', component: { template: '<div />' } },
    ],
  })
  await router.push('/repos')
  const wrapper = mount(ReposView, { global: { plugins: [createPinia(), router] } })
  await flushPromises()
  return wrapper
}
beforeEach(() => list.mockReset())
describe('repository catalogue', () => {
  it('can continue beyond the first batch rather than hiding later repositories', async () => {
    let count = 0
    list.mockImplementation(async () => ({
      items: Array.from({ length: 200 }, (_, i) => repo(`team/project-${count * 200 + i}`)),
      nextPageToken: ++count < 6 ? String(count) : '',
    }))
    const w = await show()
    expect(w.text()).not.toContain('team/project-1199')
    const more = w.findAll('button').find((b) => b.text() === 'Load more')
    expect(more).toBeDefined()
    await more!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('team/project-1199')
    expect(w.findAll('button').some((b) => b.text() === 'Load more')).toBe(false)
    w.unmount()
  })
  it('keeps already loaded repositories visible if a later page fails', async () => {
    list
      .mockResolvedValueOnce({ items: [repo('team/keep-me')], nextPageToken: 'next' })
      .mockRejectedValueOnce(new Error('connection lost'))
    const w = await show()
    expect(w.text()).toContain('team/keep-me')
    expect(w.text()).toContain('connection lost')
    w.unmount()
  })
})
