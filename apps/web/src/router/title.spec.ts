import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { createMemoryHistory, RouterView } from 'vue-router'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { createAppRouter } from './index'

enableAutoUnmount(afterEach)
vi.mock('@/stores/auth', () => ({
  useAuth: () => ({ status: 'ready', token: 'test-session', isAuthenticated: true }),
}))
const ScopedTitle = defineComponent({
  setup() {
    const { repo } = useRepoName()
    usePageTitle(() => `Files · ${repo.value}`)
    return () => h('p', 'Files')
  },
})
async function show() {
  const router = createAppRouter(createMemoryHistory())
  router.addRoute({ path: '/title/:owner/:name', component: ScopedTitle, meta: { title: 'Files' } })
  router.addRoute({
    path: '/other-title',
    component: { template: '<p>Other</p>' },
    meta: { title: 'Other' },
  })
  await router.push('/title/acme/alpha')
  const w = mount(
    { components: { RouterView }, template: '<RouterView />' },
    { global: { plugins: [router] } },
  )
  await flushPromises()
  return { router, w }
}

describe('contextual page titles', () => {
  it('retains the view title through query, hash and Back changes', async () => {
    const { router } = await show()
    expect(document.title).toBe('Files · acme/alpha · AgentGit')
    await router.push({ query: { q: 'notes' } })
    await flushPromises()
    expect(document.title).toBe('Files · acme/alpha · AgentGit')
    await router.push({ hash: '#line-3' })
    await flushPromises()
    expect(document.title).toBe('Files · acme/alpha · AgentGit')
    router.back()
    await flushPromises()
    expect(document.title).toBe('Files · acme/alpha · AgentGit')
  })

  it('sets the new page fallback and lets a new repository refine it', async () => {
    const { router } = await show()
    await router.push('/title/acme/beta')
    await flushPromises()
    expect(document.title).toBe('Files · acme/beta · AgentGit')
    await router.push('/other-title')
    await flushPromises()
    expect(document.title).toBe('Other · AgentGit')
  })

  it('sets the initial fallback even when the first path is the root', async () => {
    const router = createAppRouter(createMemoryHistory())
    router.addRoute({
      name: 'home',
      path: '/',
      component: { template: '<p>Home</p>' },
      meta: { title: 'Home' },
    })
    document.title = 'Previous page'
    await router.push('/')
    expect(document.title).toBe('Home · AgentGit')
  })
})
