import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import RolesManager from './RolesManager.vue'
import { ApiError } from '@/api/errors'
import TextField from '@/ui/TextField.vue'
import SelectField from '@/ui/SelectField.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({ grant: vi.fn(), roles: vi.fn(), ok: vi.fn() }))
const auth = reactive({ identity: 'user:admin', token: 'first-session', status: 'ready' })
vi.mock('@/api/instance', () => ({
  api: { admin: { roles: mocks.roles, grantRole: mocks.grant } },
}))
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok }) }))
beforeEach(() => {
  Object.assign(auth, { identity: 'user:admin', token: 'first-session', status: 'ready' })
  mocks.roles.mockReset().mockResolvedValue([])
  mocks.grant.mockReset().mockResolvedValue({})
  mocks.ok.mockReset()
})
async function show() {
  const w = mount(RolesManager, {
    props: { repo: 'acme/first' },
    global: {
      stubs: {
        RouterLink: { props: ['to'], template: '<a :href="to"><slot /></a>' },
        Dialog: {
          props: ['modelValue'],
          template: '<section v-if="modelValue"><slot /><slot name="footer" /></section>',
        },
      },
    },
  })
  await flushPromises()
  await w.get('[data-testid="grant-open"]').trigger('click')
  await w.get('input[placeholder="user:alice"]').setValue('user:alice')
  await w.get('select').setValue('writer')
  return w
}

describe('granting roles', () => {
  it('offers sign-in after a rejected credential instead of retrying it', async () => {
    mocks.grant.mockRejectedValueOnce(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'Expired' }),
    )
    const w = await show()
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('Sign in again before granting a role')
    expect(w.get('a[href="/signin"]').text()).toBe('Sign in')
    await w.get('form').trigger('submit')
    expect(mocks.grant).toHaveBeenCalledTimes(1)
  })

  it('does not show an old failure inside a new repository dialog', async () => {
    let fail!: (error: Error) => void
    mocks.grant.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject
        }),
    )
    const w = await show()
    await w.get('form').trigger('submit')
    await w.setProps({ repo: 'acme/second' })
    await w.get('[data-testid="grant-open"]').trigger('click')
    await w.get('input[placeholder="user:alice"]').setValue('user:bob')
    fail(new Error('Obsolete failure'))
    await flushPromises()
    expect(w.text()).not.toContain('Obsolete failure')
    expect(w.get<HTMLInputElement>('input[placeholder="user:alice"]').element.value).toBe(
      'user:bob',
    )
  })

  it('locks pending fields, prevents duplicate submits and reports the submitted binding', async () => {
    let finish!: (value: object) => void
    mocks.grant.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    await w.get('form').trigger('submit')
    expect(
      w.findAll('input, select').every((field) => field.attributes('disabled') !== undefined),
    ).toBe(true)
    await w.get('form').trigger('submit')
    expect(mocks.grant).toHaveBeenCalledTimes(1)
    w.findComponent(TextField).vm.$emit('update:modelValue', 'user:other')
    w.findComponent(SelectField).vm.$emit('update:modelValue', 'admin')
    finish({})
    await flushPromises()
    expect(mocks.grant).toHaveBeenCalledWith(
      { subject: 'user:alice', repo: 'acme/first', role: 'writer' },
      { token: 'first-session', quietUnauthenticated: true },
    )
    expect(mocks.ok).toHaveBeenCalledWith('Role granted', 'user:alice is now writer on acme/first')
  })

  it('retains the binding after failure and allows retry', async () => {
    mocks.grant.mockRejectedValueOnce(new Error('Temporary failure'))
    const w = await show()
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('Temporary failure')
    expect(w.get<HTMLInputElement>('input[placeholder="user:alice"]').element.value).toBe(
      'user:alice',
    )
    expect(w.get('select').attributes('disabled')).toBeUndefined()
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(mocks.grant).toHaveBeenCalledTimes(2)
    expect(w.find('form').exists()).toBe(false)
  })

  it.each(['repository', 'account', 'unmount'] as const)(
    'ignores a pending result after changing %s',
    async (change) => {
      let finish!: (value: object) => void
      mocks.grant.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve
          }),
      )
      const w = await show()
      await w.get('form').trigger('submit')
      if (change === 'repository') await w.setProps({ repo: 'acme/second' })
      else if (change === 'account')
        Object.assign(auth, { identity: 'user:other', token: 'new-session' })
      else w.unmount()
      await flushPromises()
      const reads = mocks.roles.mock.calls.length
      finish({})
      await flushPromises()
      expect(mocks.ok).not.toHaveBeenCalled()
      expect(mocks.roles).toHaveBeenCalledTimes(reads)
      if (change !== 'unmount') expect(w.find('form').exists()).toBe(false)
    },
  )
})
