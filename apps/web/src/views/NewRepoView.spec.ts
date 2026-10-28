import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { reactive } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/errors'
import { applyPreset, stringifyDoc } from '@/lib/policy-plain'
import NewRepoView from './NewRepoView.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({ create: vi.fn(), policy: vi.fn(), ok: vi.fn(), warn: vi.fn() }))
const auth = reactive({
  identity: 'user:alice',
  status: 'ready',
  token: 'alice-token',
  isAdminAnywhere: true,
})
vi.mock('@/api/instance', () => ({
  api: { repos: { create: mocks.create }, admin: { setPolicy: mocks.policy } },
}))
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok, warn: mocks.warn }) }))
const repo = { name: 'acme/new', default_stream: 'main' }
beforeEach(() => {
  Object.assign(auth, {
    identity: 'user:alice',
    status: 'ready',
    token: 'alice-token',
    isAdminAnywhere: true,
  })
  mocks.create.mockReset().mockResolvedValue(repo)
  mocks.policy.mockReset().mockResolvedValue({})
  mocks.ok.mockReset()
  mocks.warn.mockReset()
})
async function review() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/repos/new', component: NewRepoView },
      {
        path: '/repos/:owner/:name',
        name: 'board',
        component: { template: '<h1>Repository</h1>' },
      },
      { path: '/away', component: { template: '<h1>Away</h1>' } },
      { path: '/repos', component: { template: '<h1>Repositories</h1>' } },
      { path: '/import', component: { template: '<h1>Import</h1>' } },
      { path: '/signin', component: { template: '<h1>Sign in</h1>' } },
    ],
  })
  await router.push('/repos/new')
  const w = mount(
    { components: { RouterView }, template: '<RouterView />' },
    { global: { plugins: [router] } },
  )
  await flushPromises()
  await w.get('input[placeholder="acme/webapp"]').setValue(repo.name)
  await w.get('textarea').setValue('Our project')
  await w.get('form').trigger('submit')
  await w.get('input[value="solo"]').setValue(true)
  await w.get('form').trigger('submit')
  return { w, router }
}

describe('creating a repository', () => {
  it('ignores duplicate clicks and keeps the selected policy and credential for the follow-up', async () => {
    let finish!: (value: typeof repo) => void
    mocks.create.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await review()
    const button = w.get('[data-testid="create-repo"]')
    await Promise.all([button.trigger('click'), button.trigger('click')])
    expect(mocks.create).toHaveBeenCalledTimes(1)
    expect(mocks.create).toHaveBeenCalledWith(
      { name: repo.name, description: 'Our project' },
      { token: 'alice-token', quietUnauthenticated: true },
    )
    finish(repo)
    await flushPromises()
    expect(mocks.policy).toHaveBeenCalledWith(
      repo.name,
      'main',
      stringifyDoc(applyPreset('solo')),
      { token: 'alice-token', quietUnauthenticated: true },
    )
    expect(router.currentRoute.value.path).toBe('/repos/acme/new')
    expect(mocks.ok).toHaveBeenCalledTimes(1)
  })

  it('does not send policy or navigate back after the user leaves while create is pending', async () => {
    let finish!: (value: typeof repo) => void
    mocks.create.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    await router.push('/away')
    finish(repo)
    await flushPromises()
    expect(mocks.policy).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/away')
    expect(mocks.ok).not.toHaveBeenCalled()
  })

  it('does not continue the old operation under a different account', async () => {
    let finish!: (value: typeof repo) => void
    mocks.create.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    Object.assign(auth, { identity: 'user:bob', token: 'bob-token' })
    await flushPromises()
    finish(repo)
    await flushPromises()
    expect(mocks.policy).not.toHaveBeenCalled()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/repos/new')
    expect(w.get<HTMLInputElement>('input[placeholder="acme/webapp"]').element.value).toBe('')
  })

  it('does not redirect after leaving while policy is pending', async () => {
    let finish!: () => void
    mocks.policy.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    const { w, router } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    await flushPromises()
    expect(mocks.policy).toHaveBeenCalledTimes(1)
    await router.push('/away')
    finish()
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/away')
    expect(mocks.ok).not.toHaveBeenCalled()
  })

  it('explains partial success if the repository exists but policy could not be saved', async () => {
    mocks.policy.mockRejectedValueOnce(new Error('Policy unavailable'))
    const { w, router } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    await flushPromises()
    expect(mocks.warn).toHaveBeenCalledWith(
      'Repository created, but the rules were not saved',
      expect.stringContaining('Policy unavailable'),
    )
    expect(router.currentRoute.value.path).toBe('/repos/acme/new')
    expect(mocks.warn.mock.calls[0]![1]).toContain(repo.name)
    expect(mocks.create).toHaveBeenCalledTimes(1)
  })

  it('keeps the name and description after a failed create', async () => {
    mocks.create.mockRejectedValueOnce(new Error('Server unavailable'))
    const { w } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Server unavailable')
    expect(w.get<HTMLInputElement>('input[placeholder="acme/webapp"]').element.value).toBe(
      repo.name,
    )
    expect(w.get('textarea').element.value).toBe('Our project')
  })
  it('does not release a new account request when the old create completes', async () => {
    const finish: ((value: typeof repo) => void)[] = []
    mocks.create.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish.push(resolve)
        }),
    )
    const { w, router } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    Object.assign(auth, { identity: 'user:bob', token: 'bob-token' })
    await flushPromises()
    await w.get('input[placeholder="acme/webapp"]').setValue('acme/second')
    await w.get('form').trigger('submit')
    await w.get('form').trigger('submit')
    await w.get('[data-testid="create-repo"]').trigger('click')
    finish[0]!(repo)
    await flushPromises()
    expect(w.get('[data-testid="create-repo"]').attributes('aria-busy')).toBe('true')
    expect(mocks.policy).not.toHaveBeenCalled()
    finish[1]!({ name: 'acme/second', default_stream: 'main' })
    await flushPromises()
    expect(mocks.policy.mock.lastCall![0]).toBe('acme/second')
    expect(mocks.policy.mock.lastCall![3]).toEqual({
      token: 'bob-token',
      quietUnauthenticated: true,
    })
    expect(router.currentRoute.value.path).toBe('/repos/acme/second')
  })

  it('offers sign-in after the pinned credential is rejected', async () => {
    mocks.create.mockRejectedValueOnce(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'Credential expired' }),
    )
    const { w } = await review()
    await w.get('[data-testid="create-repo"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Credential expired')
    expect(w.findAll('a').some((link) => link.text() === 'Sign in again')).toBe(true)
    expect(mocks.policy).not.toHaveBeenCalled()
  })
})
