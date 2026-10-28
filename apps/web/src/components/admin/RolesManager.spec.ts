import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import RolesManager from './RolesManager.vue'
import { ApiError } from '@/api/errors'
import TextField from '@/ui/TextField.vue'
import SelectField from '@/ui/SelectField.vue'
import Button from '@/ui/Button.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({
  grant: vi.fn(),
  revoke: vi.fn(),
  error: vi.fn(),
  roles: vi.fn(),
  ok: vi.fn(),
}))
const auth = reactive({ identity: 'user:admin', token: 'first-session', status: 'ready' })
vi.mock('@/api/instance', () => ({
  api: { admin: { roles: mocks.roles, grantRole: mocks.grant, revokeRole: mocks.revoke } },
}))
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok, error: mocks.error }) }))
beforeEach(() => {
  Object.assign(auth, { identity: 'user:admin', token: 'first-session', status: 'ready' })
  mocks.roles.mockReset().mockResolvedValue([])
  mocks.grant.mockReset().mockResolvedValue({})
  mocks.ok.mockReset()
  mocks.error.mockReset()
  mocks.revoke.mockReset().mockResolvedValue({})
})
async function show(openGrant = true) {
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
  if (!openGrant) return w
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

describe('revoking roles', () => {
  it('does not submit a second revoke while the first is pending', async () => {
    mocks.roles.mockResolvedValue([
      {
        subject: 'user:alice',
        repo: 'acme/first',
        role: 'reader',
        created_by: 'user:admin',
        created_at_ms: 0,
      },
    ])
    let finish!: (value: object) => void
    mocks.revoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show(false)
    await w.get('[aria-label="Revoke role"]').trigger('click')
    const confirm = w
      .findAllComponents(Button)
      .find((button) => button.props('variant') === 'danger')!
    confirm.vm.$emit('click')
    confirm.vm.$emit('click')
    expect(mocks.revoke).toHaveBeenCalledTimes(1)
    finish({})
    await flushPromises()
  })

  it('does not display an old failure in a new repository confirmation', async () => {
    const binding = (repo: string) => ({
      subject: 'user:alice',
      repo,
      role: 'reader',
      created_by: 'user:admin',
      created_at_ms: 0,
    })
    mocks.roles
      .mockResolvedValueOnce([binding('acme/first')])
      .mockResolvedValue([binding('acme/second')])
    let fail!: (error: Error) => void
    mocks.revoke.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject
        }),
    )
    const w = await show(false)
    await w.get('[aria-label="Revoke role"]').trigger('click')
    await w
      .findAll('button')
      .find((button) => button.text() === 'Revoke')!
      .trigger('click')
    await w.setProps({ repo: 'acme/second' })
    await flushPromises()
    await w.get('[aria-label="Revoke role"]').trigger('click')
    fail(new Error('Old revoke failure'))
    await flushPromises()
    expect(w.text()).not.toContain('Old revoke failure')
    expect(w.text()).toContain('acme/second')
  })

  it('keeps the failed binding available for a retry with the initiating credential', async () => {
    const binding = { subject: 'user:alice', repo: 'acme/first', role: 'reader' }
    mocks.roles.mockResolvedValue([{ ...binding, created_by: 'user:admin', created_at_ms: 0 }])
    mocks.revoke.mockRejectedValueOnce(new Error('Revoke unavailable'))
    const w = await show(false)
    await w.get('[aria-label="Revoke role"]').trigger('click')
    const confirm = () => w.findAll('button').find((button) => button.text() === 'Revoke')!
    await confirm().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Revoke unavailable')
    expect(w.text()).toContain('user:alice')
    await confirm().trigger('click')
    await flushPromises()
    expect(mocks.revoke).toHaveBeenCalledTimes(2)
    expect(mocks.revoke).toHaveBeenLastCalledWith(binding, {
      token: 'first-session',
      quietUnauthenticated: true,
    })
    expect(mocks.ok).toHaveBeenCalledWith(
      'Role revoked',
      'user:alice no longer has reader on acme/first',
    )
  })

  it('explains a rejected session without resending the revoke', async () => {
    mocks.roles.mockResolvedValue([
      {
        subject: 'user:alice',
        repo: 'acme/first',
        role: 'reader',
        created_by: 'user:admin',
        created_at_ms: 0,
      },
    ])
    mocks.revoke.mockRejectedValueOnce(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'Expired' }),
    )
    const w = await show(false)
    await w.get('[aria-label="Revoke role"]').trigger('click')
    await w
      .findAll('button')
      .find((button) => button.text() === 'Revoke')!
      .trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Sign in again before revoking a role')
    expect(w.get('a[href="/signin"]').text()).toBe('Sign in')
    expect(
      w
        .findAll('button')
        .find((button) => button.text() === 'Revoke')!
        .attributes('aria-disabled'),
    ).toBe('true')
    expect(mocks.revoke).toHaveBeenCalledTimes(1)
  })

  it.each(['repository', 'account', 'unmount'] as const)(
    'ignores a pending revoke after changing %s',
    async (change) => {
      mocks.roles.mockResolvedValue([
        {
          subject: 'user:alice',
          repo: 'acme/first',
          role: 'reader',
          created_by: 'user:admin',
          created_at_ms: 0,
        },
      ])
      let finish!: (value: object) => void
      mocks.revoke.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve
          }),
      )
      const w = await show(false)
      await w.get('[aria-label="Revoke role"]').trigger('click')
      await w
        .findAll('button')
        .find((button) => button.text() === 'Revoke')!
        .trigger('click')
      if (change === 'repository') await w.setProps({ repo: 'acme/second' })
      else if (change === 'account')
        Object.assign(auth, { identity: 'user:other', token: 'other-session' })
      else w.unmount()
      await flushPromises()
      const reads = mocks.roles.mock.calls.length
      finish({})
      await flushPromises()
      expect(mocks.ok).not.toHaveBeenCalled()
      expect(mocks.roles).toHaveBeenCalledTimes(reads)
      if (change !== 'unmount')
        expect(w.findAll('button').some((button) => button.text() === 'Revoke')).toBe(false)
    },
  )
})
