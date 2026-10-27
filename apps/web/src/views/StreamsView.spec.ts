import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import StreamsView from './StreamsView.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({ create: vi.fn(), list: vi.fn(), ok: vi.fn(), touched: vi.fn() }))
vi.mock('@/api/instance', () => ({
  api: { streams: { create: mocks.create, list: mocks.list, reflog: async () => [] } },
}))
vi.mock('@/stores/auth', () => ({ useAuth: () => ({ hasRole: () => true }) }))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ repo: null, touched: mocks.touched }) }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok }) }))

beforeEach(() => {
  mocks.create.mockReset().mockResolvedValue({})
  mocks.list.mockReset().mockResolvedValue([])
  mocks.ok.mockReset()
  mocks.touched.mockReset()
})
async function open() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/repos/:owner/:name/streams', component: StreamsView }],
  })
  await router.push('/repos/acme/demo/streams')
  const w = mount(StreamsView, { global: { plugins: [router], stubs: { Teleport: true } } })
  await flushPromises()
  await w
    .findAll('button')
    .find((b) => b.text() === 'New branch')!
    .trigger('click')
  await flushPromises()
  return w
}

describe('branch creation', () => {
  it('locks submitted inputs and ignores duplicate submissions while creation is pending', async () => {
    let finish!: (value: object) => void
    mocks.create.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await open()
    const form = w.get('#new-stream')
    const inputs = form.findAll('input')
    await inputs[0]!.setValue(' topic/first ')
    await w.get('#new-stream input[placeholder="main"]').setValue(' main ')
    await form.trigger('submit')
    expect(
      w
        .get('#new-stream')
        .findAll('input')
        .every((input) => input.attributes('disabled') !== undefined),
    ).toBe(true)
    await form.trigger('submit')
    expect(mocks.create).toHaveBeenCalledTimes(1)
    expect(mocks.create).toHaveBeenCalledWith('acme/demo', {
      name: 'topic/first',
      from_rev: 'main',
    })
    // Even programmatic changes cannot rename the request that already left the browser.
    w.findAllComponents({ name: 'TextField' })
      .find((field) => field.props('label') === 'Name')!
      .vm.$emit('update:modelValue', 'topic/second')
    await flushPromises()
    finish({})
    await flushPromises()
    expect(mocks.ok).toHaveBeenCalledWith('Stream created', 'topic/first')
    expect(w.find('#new-stream').exists()).toBe(false)
  })

  it('keeps the entered branch and revision after failure and allows a corrected retry', async () => {
    mocks.create.mockRejectedValueOnce(new Error('Creation failed'))
    const w = await open()
    let form = w.get('#new-stream')
    let inputs = form.findAll('input')
    await inputs[0]!.setValue('topic/first')
    await w.get('#new-stream input[placeholder="main"]').setValue('main')
    await form.trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('Creation failed')
    inputs = w.get('#new-stream').findAll('input')
    expect(inputs[0]!.element.value).toBe('topic/first')
    expect(inputs[1]!.element.value).toBe('main')
    expect(inputs.every((input) => input.attributes('disabled') === undefined)).toBe(true)
    form = w.get('#new-stream')
    inputs = form.findAll('input')
    await inputs[0]!.setValue('topic/corrected')
    await form.trigger('submit')
    await flushPromises()
    expect(mocks.create).toHaveBeenLastCalledWith('acme/demo', {
      name: 'topic/corrected',
      from_rev: 'main',
    })
    expect(mocks.ok).toHaveBeenCalledWith('Stream created', 'topic/corrected')
  })
})
