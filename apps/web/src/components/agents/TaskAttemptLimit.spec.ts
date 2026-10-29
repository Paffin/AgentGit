import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/errors'
import type { Task } from '@/api/types'
import TaskAttemptLimit from './TaskAttemptLimit.vue'

enableAutoUnmount(afterEach)
const setTaskAttemptLimit = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { agents: { setTaskAttemptLimit } } }))
const auth = reactive({
  token: 'session',
  identity: 'user:alice',
  capabilities: { features: ['tasks.attempt_limits'] },
  can: (_repo: string, action: string): boolean => action === 'createTask',
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const task = (): Task => ({
  id: 'task-1',
  title: 'Work',
  description: '',
  scope: [],
  owner: '',
  state: 'open',
  created_by: 'user:alice',
  created_at_ms: 1,
  updated_at_ms: 1,
  depends_on: [],
  ready: true,
  max_attempts: 2,
  attempts_started: 0,
})
function show(value = task()) {
  return mount(TaskAttemptLimit, {
    props: { repo: 'acme/demo', task: value },
    global: { stubs: { Teleport: true } },
  })
}
async function edit(wrapper: ReturnType<typeof show>, value: string) {
  await wrapper.get('button').trigger('click')
  await wrapper.get('input').setValue(value)
}
beforeEach(() => {
  auth.token = 'session'
  auth.identity = 'user:alice'
  auth.capabilities.features = ['tasks.attempt_limits']
  auth.can = (_repo, action) => action === 'createTask'
  setTaskAttemptLimit.mockReset().mockResolvedValue({ ...task(), max_attempts: 3 })
})
describe('task attempt limit', () => {
  it('requires confirmation and sends the inspected limit with the new value', async () => {
    const wrapper = show()
    await edit(wrapper, '3')
    expect(setTaskAttemptLimit).not.toHaveBeenCalled()
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(setTaskAttemptLimit).toHaveBeenCalledWith(
      'acme/demo',
      'task-1',
      3,
      2,
      expect.objectContaining({ token: 'session', idempotencyKey: expect.any(String) }),
    )
    expect(wrapper.emitted('saved')?.[0]?.[0]).toMatchObject({ max_attempts: 3 })
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
  })
  it('accepts zero and the server maximum without losing the inspected limit', async () => {
    for (const value of ['0', '10000']) {
      const wrapper = show()
      await edit(wrapper, value)
      await wrapper.get('form').trigger('submit')
      await flushPromises()
      expect(setTaskAttemptLimit.mock.lastCall?.slice(2, 4)).toEqual([Number(value), 2])
      wrapper.unmount()
    }
  })
  it('keeps a retry key stable after a network failure and changes it for a new value', async () => {
    setTaskAttemptLimit.mockRejectedValue(
      new ApiError({ status: 0, code: 'network_error', message: 'Offline' }),
    )
    const wrapper = show()
    await edit(wrapper, '3')
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    const key = setTaskAttemptLimit.mock.lastCall?.[4].idempotencyKey
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(setTaskAttemptLimit.mock.lastCall?.[4].idempotencyKey).toBe(key)
    await wrapper.get('input').setValue('4')
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(setTaskAttemptLimit.mock.lastCall?.[4].idempotencyKey).not.toBe(key)
  })
  it.each(['', '-1', '1.5', '10001'])('does not submit invalid limit %s', async (value) => {
    const wrapper = show()
    await edit(wrapper, value)
    await wrapper.get('form').trigger('submit')
    expect(setTaskAttemptLimit).not.toHaveBeenCalled()
  })
  it('keeps a stale draft without silently rebasing its precondition', async () => {
    setTaskAttemptLimit.mockRejectedValue(
      new ApiError({ status: 412, code: 'failed_precondition', message: 'Reload before saving' }),
    )
    const wrapper = show()
    await edit(wrapper, '3')
    await wrapper.setProps({ task: { ...task(), max_attempts: 5 } })
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(setTaskAttemptLimit.mock.lastCall?.[3]).toBe(2)
    expect(wrapper.get('input').element.value).toBe('3')
    expect(wrapper.text()).toContain('Reload before saving')
    expect(wrapper.emitted('reload')).toHaveLength(1)
    await wrapper.get('form').trigger('submit')
    expect(setTaskAttemptLimit).toHaveBeenCalledTimes(1)
  })
  it('requires an operator after execution starts and hides finished or unsupported edits', async () => {
    const wrapper = show({ ...task(), attempts_started: 1 })
    expect(wrapper.find('button').exists()).toBe(false)
    auth.can = () => true
    await flushPromises()
    expect(wrapper.find('button').exists()).toBe(true)
    await wrapper.setProps({ task: { ...task(), state: 'done' } })
    expect(wrapper.find('button').exists()).toBe(false)
    await wrapper.setProps({ task: task() })
    auth.capabilities.features = []
    await flushPromises()
    expect(wrapper.find('button').exists()).toBe(false)
  })
  it('discards an old session response and closes its draft', async () => {
    let finish!: (value: Task) => void
    setTaskAttemptLimit.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = show()
    await edit(wrapper, '3')
    await wrapper.get('form').trigger('submit')
    const signal = setTaskAttemptLimit.mock.lastCall?.[4].signal as AbortSignal
    auth.token = 'different-session'
    await flushPromises()
    expect(signal.aborted).toBe(true)
    finish(task())
    await flushPromises()
    expect(wrapper.emitted('saved')).toBeUndefined()
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
  })
})
