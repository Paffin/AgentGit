import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import FileEditor from './FileEditor.vue'
import { ApiError } from '@/api/errors'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({ propose: vi.fn(), touched: vi.fn(), ok: vi.fn() }))
const auth = reactive({ identity: 'user:alice', token: 'alice-token', status: 'ready' })
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ touched: mocks.touched }) }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok }) }))
vi.mock('@/lib/edit', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  proposeChange: mocks.propose,
}))

beforeEach(() => {
  sessionStorage.clear()
  Object.assign(auth, { identity: 'user:alice', token: 'alice-token', status: 'ready' })
  mocks.propose.mockReset()
  mocks.touched.mockReset()
  mocks.ok.mockReset()
})
async function editor() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path: '/edit',
        component: FileEditor,
        props: {
          repo: 'acme/app',
          path: 'README.md',
          original: 'Original',
          rev: 'base',
          stream: 'main',
        },
      },
      { path: '/proposals/:id', name: 'proposal', component: { template: '<h1>Proposal</h1>' } },
    ],
  })
  await router.push('/edit')
  const w = mount(RouterView, {
    attachTo: document.body,
    global: { plugins: [router], stubs: { Teleport: true } },
  })
  await flushPromises()
  await w.get('textarea').setValue('Edited text')
  await w.get('[data-testid="propose-change"]').trigger('click')
  await flushPromises()
  const push = vi.spyOn(router, 'push')
  return { w, router, push }
}

describe('file editor pending submission', () => {
  it('submits once and locks the submitted title and description', async () => {
    mocks.propose.mockReturnValue(new Promise(() => {}))
    const { w } = await editor()
    await w.get('#propose-form').trigger('submit')
    await w.get('#propose-form').trigger('submit')
    expect(mocks.propose).toHaveBeenCalledTimes(1)
    expect(w.get('#propose-form input').attributes('disabled')).toBeDefined()
    expect(w.get('#propose-form textarea').attributes('disabled')).toBeDefined()
    expect(mocks.propose.mock.calls[0]![1].request).toMatchObject({
      token: 'alice-token',
      quietUnauthenticated: true,
    })
  })

  it('keeps the draft and ignores a late result after unmount', async () => {
    let finish!: (value: object) => void
    mocks.propose.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, push } = await editor()
    await w.get('#propose-form').trigger('submit')
    const request = mocks.propose.mock.calls[0]![1].request
    w.unmount()
    finish({ id: 'old', title: 'Old result' })
    await flushPromises()
    expect(request.signal.aborted).toBe(true)
    expect(push).not.toHaveBeenCalled()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(Object.values(sessionStorage).join(' ')).toContain('Edited text')
  })

  it('cancels the old request and closes the editor when the account changes', async () => {
    let finish!: (value: object) => void
    mocks.propose.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, push } = await editor()
    const component = w.findComponent(FileEditor)
    await w.get('#propose-form').trigger('submit')
    const request = mocks.propose.mock.calls[0]![1].request
    Object.assign(auth, { identity: 'user:bob', token: 'bob-token' })
    await flushPromises()
    finish({ id: 'old', title: 'Old result' })
    await flushPromises()
    expect(request.signal.aborted).toBe(true)
    expect(component.emitted('cancel')).toHaveLength(1)
    expect(push).not.toHaveBeenCalled()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(Object.values(sessionStorage).join(' ')).toContain('Edited text')
  })
  it('keeps the draft and offers sign-in when the initiating credential is rejected', async () => {
    mocks.propose.mockRejectedValue(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'Credential expired' }),
    )
    const { w } = await editor()
    await w.get('#propose-form').trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('Credential expired')
    expect(w.text()).toContain('Sign in again')
    expect(w.get('[data-testid="propose-submit"]').attributes('aria-disabled')).toBe('true')
    await w.get('#propose-form').trigger('submit')
    expect(mocks.propose).toHaveBeenCalledTimes(1)
    expect(Object.values(sessionStorage).join(' ')).toContain('Edited text')
  })
})
