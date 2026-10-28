import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { reactive } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import CodeSearchView from './CodeSearchView.vue'

enableAutoUnmount(afterEach)
const search = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { tree: { search } } }))
const auth = reactive({
  token: 'alice-token',
  identity: 'user:alice',
  status: 'ready',
  capabilities: { features: ['tree.search'] },
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ repo: { default_stream: 'main' } }) }))
const response = (text = 'private needle') => ({
  rev: 'revision',
  tree_id: 'tree',
  matches: [{ path: 'src.txt', line: 1, text }],
  next_page_token: '',
  files_scanned: 1,
  skipped_binary: 0,
  skipped_large: 0,
  scanned_bytes: 20,
})
async function show() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos/:owner/:name/search', component: CodeSearchView },
      { path: '/repos/:owner/:name/files', name: 'files', component: { template: '<div />' } },
    ],
  })
  await router.push('/repos/acme/demo/search?q=needle')
  const wrapper = mount(CodeSearchView, { global: { plugins: [router] } })
  await flushPromises()
  return { wrapper, router }
}
beforeEach(() => {
  Object.assign(auth, { token: 'alice-token', identity: 'user:alice', status: 'ready' })
  search.mockReset().mockResolvedValue(response())
})
describe('code search session and literal text', () => {
  it('clears the previous account results and ignores its late response', async () => {
    let finish!: (value: ReturnType<typeof response>) => void
    search.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { wrapper } = await show()
    const oldSignal = search.mock.calls[0]![2].signal as AbortSignal
    auth.token = 'bob-token'
    auth.identity = 'user:bob'
    await flushPromises()
    expect(oldSignal.aborted).toBe(true)
    expect(search).toHaveBeenLastCalledWith(
      'acme/demo',
      expect.anything(),
      expect.objectContaining({ token: 'bob-token' }),
    )
    finish(response('alice-only source'))
    await flushPromises()
    expect(wrapper.text()).not.toContain('alice-only source')
  })
  it('removes displayed results on sign out without issuing anonymous search', async () => {
    const { wrapper } = await show()
    expect(wrapper.text()).toContain('private needle')
    auth.token = ''
    auth.identity = ''
    auth.status = 'anonymous'
    await flushPromises()
    expect(wrapper.text()).not.toContain('private needle')
    expect(search).toHaveBeenCalledTimes(1)
  })
  it('preserves intentional spaces in a literal query', async () => {
    const { wrapper, router } = await show()
    await wrapper.get('input[type="search"]').setValue('  needle ')
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(router.currentRoute.value.query.q).toBe('  needle ')
    expect(search.mock.lastCall![1].query).toBe('  needle ')
  })
})
