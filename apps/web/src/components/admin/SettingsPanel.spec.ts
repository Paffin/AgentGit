import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import SettingsPanel from './SettingsPanel.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  configure: vi.fn(),
  ok: vi.fn(),
  refresh: vi.fn(),
}))
vi.mock('@/api/instance', () => ({
  api: { repos: { get: mocks.get, configure: mocks.configure } },
}))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ refreshAll: mocks.refresh }) }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok }) }))
const info = (description: string) => ({
  description,
  quota: {},
  namespace: 'team',
  meta_backend: 'sqlite',
  object_backend: 'local',
  durability: 'fsync',
  stream_count: 1,
  changeset_count: 1,
  created_by: 'alice',
})
beforeEach(() => {
  mocks.get.mockReset().mockImplementation(async (repo: string) => info(repo))
  mocks.configure.mockReset().mockResolvedValue({})
  mocks.ok.mockReset()
  mocks.refresh.mockReset()
})
async function show() {
  const w = mount(SettingsPanel, { props: { repo: 'acme/first' } })
  await flushPromises()
  return w
}

describe('repository settings saves', () => {
  it('locks pending values, ignores duplicate submissions and preserves them after failure for retry', async () => {
    let reject!: (error: Error) => void
    mocks.configure.mockImplementationOnce(
      () =>
        new Promise((_resolve, fail) => {
          reject = fail
        }),
    )
    const w = await show()
    await w.get('textarea').setValue('New description')
    await w.findAll('input')[0]!.setValue('4096')
    await w.get('form').trigger('submit')
    expect(w.get('textarea').attributes('disabled')).toBeDefined()
    expect(w.findAll('input').every((field) => field.attributes('disabled') !== undefined)).toBe(
      true,
    )
    await w.get('form').trigger('submit')
    expect(mocks.configure).toHaveBeenCalledTimes(1)
    reject(new Error('Save unavailable'))
    await flushPromises()
    expect(w.text()).toContain('Save unavailable')
    expect(w.get('textarea').element.value).toBe('New description')
    expect(w.findAll('input')[0]!.element.value).toBe('4096')
    expect(w.get('textarea').attributes('disabled')).toBeUndefined()
    mocks.get.mockResolvedValueOnce(info('Saved description'))
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(mocks.configure).toHaveBeenLastCalledWith(
      'acme/first',
      expect.objectContaining({
        description: 'New description',
        quota: expect.objectContaining({ max_total_object_bytes: 4096 }),
      }),
    )
    expect(w.get('textarea').element.value).toBe('Saved description')
    expect(mocks.ok).toHaveBeenCalledTimes(1)
  })

  it.each(['resolve', 'reject'] as const)(
    'ignores an old save %s after changing repository',
    async (outcome) => {
      let finish!: (value: object) => void
      let fail!: (error: Error) => void
      mocks.configure.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finish = resolve
            fail = reject
          }),
      )
      const w = await show()
      await w.get('form').trigger('submit')
      await w.setProps({ repo: 'acme/second' })
      await flushPromises()
      await w.get('textarea').setValue('Second draft')
      expect(w.get('textarea').attributes('disabled')).toBeUndefined()
      if (outcome === 'resolve') finish({})
      else fail(new Error('Old save failed'))
      await flushPromises()
      expect(w.get('textarea').element.value).toBe('Second draft')
      expect(w.text()).not.toContain('Old save failed')
      expect(mocks.get).toHaveBeenCalledTimes(2)
      expect(mocks.ok).not.toHaveBeenCalled()
      expect(mocks.refresh).not.toHaveBeenCalled()
    },
  )

  it('does not release a new save when the previous repository request finishes', async () => {
    const finish: ((value: object) => void)[] = []
    mocks.configure.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish.push(resolve)
        }),
    )
    const w = await show()
    await w.get('form').trigger('submit')
    await w.setProps({ repo: 'acme/second' })
    await flushPromises()
    await w.get('textarea').setValue('Second saved description')
    await w.get('form').trigger('submit')
    finish[0]!({})
    await flushPromises()
    expect(w.get('textarea').attributes('disabled')).toBeDefined()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(mocks.get).toHaveBeenCalledTimes(2)
    finish[1]!({})
    await flushPromises()
    expect(w.get('textarea').attributes('disabled')).toBeUndefined()
    expect(mocks.configure).toHaveBeenLastCalledWith(
      'acme/second',
      expect.objectContaining({ description: 'Second saved description' }),
    )
    expect(mocks.ok).toHaveBeenCalledTimes(1)
  })

  it('rejects invalid quota without sending a partial configuration', async () => {
    const w = await show()
    await w.findAll('input')[0]!.setValue('-1')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('must be a whole number, 0 or more')
    expect(mocks.configure).not.toHaveBeenCalled()
    expect(w.findAll('input')[0]!.element.value).toBe('-1')
  })

  it('ignores a completed save after leaving the settings panel', async () => {
    let finish!: (value: object) => void
    mocks.configure.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    await w.get('form').trigger('submit')
    w.unmount()
    finish({})
    await flushPromises()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(mocks.get).toHaveBeenCalledTimes(1)
    expect(mocks.refresh).not.toHaveBeenCalled()
  })
})
