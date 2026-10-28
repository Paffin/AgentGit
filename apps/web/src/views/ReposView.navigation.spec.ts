import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import ReposView from './ReposView.vue'

enableAutoUnmount(afterEach)
const list = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { repos: { list } } }))
vi.mock('@/stores/auth', () => ({ useAuth: () => ({ isAdminAnywhere: true }) }))
beforeEach(() => {
  list.mockReset().mockResolvedValue({
    items: ['acme/first', 'acme/second'].map((name) => ({
      name,
      description: '',
      default_stream: 'main',
      stream_count: 1,
      changeset_count: 1,
      created_at_ms: 0,
    })),
    nextPageToken: '',
  })
})
async function show(path = '/repos') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos', component: ReposView },
      { path: '/repos/new', component: { template: '<i />' } },
      {
        path: '/repos/:owner/:name',
        name: 'board',
        component: { template: '<h1>Repository</h1>' },
      },
      { path: '/import', component: { template: '<i />' } },
    ],
  })
  await router.push(path)
  const w = mount(
    { components: { RouterView }, template: '<RouterView />' },
    {
      global: { plugins: [router] },
    },
  )
  await flushPromises()
  return { w, router }
}
const input = 'input[type="search"]'
const names = (w: Awaited<ReturnType<typeof show>>['w']) =>
  w.findAll('a.repo-link').map((a) => a.text())

describe('repository search navigation', () => {
  it('follows Back and Forward query changes and rejects repeated query values', async () => {
    const { w, router } = await show('/repos?q=first')
    await router.push('/repos?q=second')
    expect(names(w)).toEqual(['acme/second'])
    router.back()
    await flushPromises()
    expect(names(w)).toEqual(['acme/first'])
    router.forward()
    await flushPromises()
    expect(w.get<HTMLInputElement>(input).element.value).toBe('second')
    await router.push('/repos?q=first&q=second')
    expect(w.get<HTMLInputElement>(input).element.value).toBe('')
    expect(names(w)).toHaveLength(2)
    expect(list).toHaveBeenCalledTimes(1)
  })

  it('opens a bookmarked filter without fetching a separate search result', async () => {
    const { w, router } = await show('/repos?q=second&view=compact')
    expect(w.get<HTMLInputElement>(input).element.value).toBe('second')
    expect(names(w)).toEqual(['acme/second'])
    await w.get(input).setValue('first')
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({ q: 'first', view: 'compact' })
    expect(list).toHaveBeenCalledTimes(1)
    await w.get(input).setValue('')
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({ view: 'compact' })
    expect(names(w)).toEqual(['acme/first', 'acme/second'])
  })

  it('restores the query and visible repository after opening one and going back', async () => {
    const { w, router } = await show()
    await w.get(input).setValue('second')
    await flushPromises()
    await w.get('a.repo-link').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.name).toBe('board')
    router.back()
    await flushPromises()
    expect(w.get<HTMLInputElement>(input).element.value).toBe('second')
    expect(names(w)).toEqual(['acme/second'])
  })
})
