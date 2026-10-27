import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { IntegrationOutcome, Proposal } from '@/api/types'
import IntegrateDialog from './IntegrateDialog.vue'

enableAutoUnmount(afterEach)
const mocks = vi.hoisted(() => ({ integrate: vi.fn(), ok: vi.fn() }))
vi.mock('@/api/instance', () => ({ api: { integration: { integrate: mocks.integrate } } }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: mocks.ok }) }))
const proposal: Proposal = {
  id: 'first',
  head: 'head',
  author: 'user:alice',
  created_at_ms: 0,
  updated_at_ms: 0,
  description: '',
  integrated_as: '',
  state: 'open',
  stream: 'main',
  task_id: '',
  title: 'First proposal',
}
const outcome: IntegrationOutcome = {
  base: '',
  blockers: [],
  candidate_changeset: '',
  candidate_tree: '',
  changed_files: 1,
  conflicts: [],
  new_head: 'landed-head',
  policy_version: 1,
  previous_head: 'old-head',
  reason: '',
  status: 'integrated',
  strategy: 'fast_forward',
  tree_id: 'tree',
  version: 2,
}
beforeEach(() => {
  mocks.integrate.mockReset()
  mocks.ok.mockReset()
})
function show() {
  return mount(IntegrateDialog, {
    props: { modelValue: true, repo: 'acme/demo', proposal, authorLeases: [] },
    global: {
      stubs: {
        Dialog: {
          props: ['modelValue'],
          template: '<section v-if="modelValue"><slot/><slot name="footer"/></section>',
        },
      },
    },
  })
}
function deferred() {
  let resolve!: (value: { outcome: IntegrationOutcome }) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<{ outcome: IntegrationOutcome }>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

describe('pending integration', () => {
  it('sends one snapshot and locks reservation keys until the result arrives', async () => {
    const pending = deferred()
    mocks.integrate.mockReturnValueOnce(pending.promise)
    const w = show()
    await w.get('input').setValue('7, 9')
    const land = w.findAll('button').find((button) => button.text() === 'Land')!
    await Promise.all([land.trigger('click'), land.trigger('click')])
    expect(mocks.integrate).toHaveBeenCalledTimes(1)
    expect(mocks.integrate).toHaveBeenCalledWith('acme/demo', 'first', [7, 9])
    expect(w.get('input').attributes('disabled')).toBeDefined()
    pending.resolve({ outcome })
    await flushPromises()
    expect(w.text()).toContain('main is now at')
    expect(mocks.ok).toHaveBeenCalledWith('Landed', 'main is now at landed-hea')
    expect(w.emitted('done')).toEqual([[]])
  })

  it('preserves keys on failure and lets the user correct them for retry', async () => {
    mocks.integrate
      .mockRejectedValueOnce(new Error('Connection lost'))
      .mockResolvedValueOnce({ outcome })
    const w = show()
    await w.get('input').setValue('7')
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Connection lost')
    expect(w.get('input').element.value).toBe('7')
    expect(w.get('input').attributes('disabled')).toBeUndefined()
    await w.get('input').setValue('9')
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    await flushPromises()
    expect(mocks.integrate).toHaveBeenLastCalledWith('acme/demo', 'first', [9])
    expect(w.emitted('done')).toEqual([[]])
  })

  it.each([
    ['proposal', 'resolve'],
    ['proposal', 'reject'],
    ['repository', 'resolve'],
    ['repository', 'reject'],
    ['close', 'resolve'],
    ['close', 'reject'],
  ] as const)('ignores an old integration after %s (%s)', async (context, result) => {
    const pending = deferred()
    mocks.integrate.mockReturnValueOnce(pending.promise)
    const w = show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    if (context === 'proposal')
      await w.setProps({ proposal: { ...proposal, id: 'second', stream: 'release' } })
    else if (context === 'repository') await w.setProps({ repo: 'other/repository' })
    else await w.setProps({ modelValue: false })
    if (result === 'resolve') pending.resolve({ outcome })
    else pending.reject(new Error('Old operation failed'))
    await flushPromises()
    expect(w.find('.result').exists()).toBe(false)
    expect(w.text()).not.toContain('Old operation failed')
    if (context !== 'close') expect(w.get('input').attributes('disabled')).toBeUndefined()
    expect(w.emitted('done')).toBeUndefined()
    expect(mocks.ok).not.toHaveBeenCalled()
  })

  it('does not unlock a new integration when the old request finishes', async () => {
    const first = deferred()
    const second = deferred()
    mocks.integrate.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const w = show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    await w.setProps({ proposal: { ...proposal, id: 'second', stream: 'release' } })
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    first.resolve({ outcome })
    await flushPromises()
    expect(w.get('input').attributes('disabled')).toBeDefined()
    expect(mocks.ok).not.toHaveBeenCalled()
    second.resolve({ outcome })
    await flushPromises()
    expect(mocks.ok).toHaveBeenCalledWith('Landed', 'release is now at landed-hea')
    expect(w.emitted('done')).toEqual([[]])
  })

  it('reports a missing outcome without claiming success', async () => {
    mocks.integrate.mockResolvedValueOnce({ outcome: null })
    const w = show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    await flushPromises()
    expect(w.text()).toContain('The server answered without an outcome.')
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(w.emitted('done')).toBeUndefined()
  })

  it('shows a blocked outcome and allows another attempt', async () => {
    mocks.integrate.mockResolvedValueOnce({ outcome: { ...outcome, status: 'blocked' } })
    const w = show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Blocked: the branch was not changed')
    expect(w.get('input').attributes('disabled')).toBeUndefined()
    expect(mocks.ok).not.toHaveBeenCalled()
    expect(w.emitted('done')).toEqual([[]])
  })

  it('does not announce a completed operation after unmount', async () => {
    const pending = deferred()
    mocks.integrate.mockReturnValueOnce(pending.promise)
    const w = show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Land')!
      .trigger('click')
    w.unmount()
    pending.resolve({ outcome })
    await flushPromises()
    expect(w.emitted('done')).toBeUndefined()
    expect(mocks.ok).not.toHaveBeenCalled()
  })
})
