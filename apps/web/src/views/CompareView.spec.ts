import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import CompareView from './CompareView.vue'

const mocks = vi.hoisted(() => ({ resolve: vi.fn(), diff: vi.fn() }))
vi.mock('@/api/instance', () => ({
  api: { history: { resolve: mocks.resolve }, tree: { diff: mocks.diff } },
}))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ streams: [] }) }))
async function show(path = '/repos/acme/demo/compare?to=right') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/repos/:owner/:name/compare', component: CompareView }],
  })
  await router.push(path)
  const w = mount(CompareView, { global: { plugins: [router] } })
  await flushPromises()
  return { w, router }
}
beforeEach(() => {
  mocks.resolve.mockReset().mockImplementation(async (_repo, revision) => `id-${revision}`)
  mocks.diff.mockReset().mockResolvedValue({ files: [], truncated: false })
})

describe('comparison semantics', () => {
  it('describes blank From as the target first parent instead of the empty tree', async () => {
    const { w } = await show()
    expect(w.text()).toContain('first parent')
    expect(w.text()).not.toContain('empty tree')
    expect(w.get('.ids').text()).toContain('Changes introduced by id-right')
    expect(mocks.resolve).toHaveBeenCalledTimes(1)
    expect(mocks.diff).toHaveBeenCalledWith(
      'acme/demo',
      { from: '', to: 'id-right', context: 3 },
      expect.anything(),
    )
    const swap = w.findAll('button').find((button) => button.text().startsWith('Swap revisions'))!
    expect(swap.attributes('aria-disabled')).toBe('true')
    expect(swap.attributes('title')).toContain('Enter both')
    w.unmount()
  })

  it('asks for a target when To is empty, even if From is supplied', async () => {
    const { w } = await show('/repos/acme/demo/compare?from=left')
    expect(w.text()).toContain('Choose a target revision')
    expect(w.text()).not.toContain('Pick two revisions')
    expect(w.find('.ids').exists()).toBe(false)
    expect(mocks.diff).not.toHaveBeenCalled()
    expect(mocks.resolve).not.toHaveBeenCalled()
    w.unmount()
  })

  it('swaps explicit revisions once and restores them through browser history', async () => {
    const { w, router } = await show('/repos/acme/demo/compare?from=left&to=right')
    const fields = w.findAll('input')
    expect(w.get('.ids').text()).toBe('id-left → id-right')
    await w
      .findAll('button')
      .find((button) => button.text().startsWith('Swap revisions'))!
      .trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({ from: 'right', to: 'left' })
    expect(fields[0]!.element.value).toBe('right')
    expect(fields[1]!.element.value).toBe('left')
    expect(w.get('.ids').text()).toBe('id-right → id-left')
    expect(mocks.diff).toHaveBeenCalledTimes(2)
    router.back()
    await flushPromises()
    expect(fields[0]!.element.value).toBe('left')
    expect(fields[1]!.element.value).toBe('right')
    expect(w.get('.ids').text()).toBe('id-left → id-right')
    w.unmount()
  })

  it('clears From to choose the implicit parent and restores explicit From on Back', async () => {
    const { w, router } = await show('/repos/acme/demo/compare?from=left&to=right')
    await w.findAll('input')[0]!.setValue('')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(router.currentRoute.value.query.from).toBeUndefined()
    expect(w.get('.ids').text()).toBe('Changes introduced by id-right')
    router.back()
    await flushPromises()
    expect(w.findAll('input')[0]!.element.value).toBe('left')
    expect(w.get('.ids').text()).toBe('id-left → id-right')
    w.unmount()
  })
})
