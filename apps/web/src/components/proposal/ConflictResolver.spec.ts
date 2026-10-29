import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { Proposal } from '@/api/types'
import { ApiError } from '@/api/errors'
import ConflictResolver from './ConflictResolver.vue'

enableAutoUnmount(afterEach)
const api = vi.hoisted(() => ({
  workspaces: { create: vi.fn(), sync: vi.fn(), remove: vi.fn() },
  tree: { resolve: vi.fn(), file: vi.fn() },
  proposals: { update: vi.fn() },
}))
vi.mock('@/api/instance', () => ({ api }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: vi.fn() }) }))
const auth = reactive({
  token: 'original',
  identity: 'user:alice',
  capabilities: { features: ['proposals.update_head', 'workspaces.sync_precondition'] },
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const proposal: Proposal = {
  id: 'p',
  title: 'Review',
  description: '',
  stream: 'main',
  head: 'mine-head',
  author: 'user:alice',
  state: 'open',
  task_id: '',
  integrated_as: '',
  created_at_ms: 1,
  updated_at_ms: 1,
}
const show = () =>
  mount(ConflictResolver, {
    props: { modelValue: true, repo: 'acme/app', proposal },
    global: { stubs: { Teleport: true } },
  })
beforeEach(() => {
  auth.token = 'original'
  auth.identity = 'user:alice'
  auth.capabilities.features = ['proposals.update_head', 'workspaces.sync_precondition']
  api.workspaces.create.mockReset().mockResolvedValue({ head: 'mine-head' })
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      head: '',
      stream_head: 'branch-head',
      conflicts: [{ path: 'asset.bin', kind: 'binary', message: '' }],
    })
    .mockResolvedValue({ status: 'merged', head: 'merged-head' })
  api.workspaces.remove.mockReset().mockResolvedValue({})
  api.tree.resolve.mockReset().mockImplementation(async (_repo, head) => ({
    kind: 'regular',
    entry: {
      path: 'asset.bin',
      blob_id: head === 'mine-head' ? 'mine-blob' : 'branch-blob',
      file_type: 'regular',
      executable: head === 'mine-head',
      size: 3,
    },
  }))
  api.tree.file
    .mockReset()
    .mockResolvedValue({ bytes: new Uint8Array([0, 255, 0]), truncated: false })
  api.proposals.update.mockReset().mockResolvedValue({})
})
it('requires an explicit binary choice and publishes its original blob and mode against the pinned branch', async () => {
  const w = show()
  await flushPromises()
  expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
  expect(w.find('textarea').exists()).toBe(false)
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep mine')!
    .trigger('click')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(api.workspaces.sync.mock.calls[1]![2]).toEqual([
    { path: 'asset.bin', op: 'put', blob_id: 'mine-blob', file_type: 'regular', executable: true },
  ])
  expect(api.workspaces.sync.mock.calls[1]![3]).toMatchObject({
    token: 'original',
    expectedStreamHead: 'branch-head',
  })
  expect(api.proposals.update).toHaveBeenCalledWith(
    'acme/app',
    'p',
    'merged-head',
    'mine-head',
    expect.objectContaining({ token: 'original' }),
  )
})
it('does not turn an unreadable version into a delete choice', async () => {
  api.tree.resolve.mockRejectedValue(
    new ApiError({ status: 403, code: 'permission_denied', message: 'Not allowed to read' }),
  )
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Not allowed to read')
  expect(w.findAll('button').some((b) => b.text() === 'Delete the file')).toBe(false)
  expect(api.proposals.update).not.toHaveBeenCalled()
})
it('cancels stale preparation and cleans up with the original token after its create finishes', async () => {
  let finish!: (value: object) => void
  api.workspaces.create.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const w = show()
  await flushPromises()
  auth.token = 'new-session'
  await flushPromises()
  finish({ head: 'mine-head' })
  await flushPromises()
  expect(api.workspaces.sync).not.toHaveBeenCalled()
  expect(api.workspaces.remove).toHaveBeenCalledWith(
    'acme/app',
    expect.any(String),
    expect.objectContaining({ token: 'original' }),
  )
  expect(w.emitted('done')).toBeUndefined()
})

it('preserves selections and blocks retry when the inspected branch moved', async () => {
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [{ path: 'asset.bin', kind: 'binary' }],
    })
    .mockRejectedValue(
      new ApiError({
        status: 412,
        code: 'failed_precondition',
        message: 'branch head changed: inspect again',
      }),
    )
  const w = show()
  await flushPromises()
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep mine')!
    .trigger('click')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(w.text()).toContain('Your resolution has not replaced the newer changes')
  expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
  expect(api.proposals.update).not.toHaveBeenCalled()
})
it('refuses unpinned servers before creating a workspace', async () => {
  auth.capabilities.features = ['proposals.update_head']
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Upgrade the server')
  expect(api.workspaces.create).not.toHaveBeenCalled()
})

it('does not move the proposal when a pending save finishes after the session changes', async () => {
  let finish!: (value: object) => void
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [{ path: 'asset.bin', kind: 'binary' }],
    })
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
  const w = show()
  await flushPromises()
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep mine')!
    .trigger('click')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  auth.token = 'new-session'
  await flushPromises()
  finish({ status: 'merged', head: 'merged-head' })
  await flushPromises()
  expect(api.proposals.update).not.toHaveBeenCalled()
  expect(api.workspaces.remove).toHaveBeenCalledWith(
    'acme/app',
    expect.any(String),
    expect.objectContaining({ token: 'original' }),
  )
})

it('retains prepared choices after the proposal advances and submits the original head precondition', async () => {
  api.proposals.update.mockRejectedValue(
    new ApiError({ status: 412, code: 'failed_precondition', message: 'proposal head changed' }),
  )
  const w = show()
  await flushPromises()
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep mine')!
    .trigger('click')
  await w.setProps({ proposal: { ...proposal, head: 'concurrent-head' } })
  expect(w.find('[data-testid=resolver]').exists()).toBe(true)
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(api.proposals.update).toHaveBeenCalledWith(
    'acme/app',
    'p',
    'merged-head',
    'mine-head',
    expect.anything(),
  )
  expect(w.text()).toContain('Your resolution has not replaced the newer changes')
  expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
})

it('never removes an existing workspace when creation reports a name collision', async () => {
  api.workspaces.create.mockRejectedValue(
    new ApiError({ status: 409, code: 'already_exists', message: 'Workspace already exists' }),
  )
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Workspace already exists')
  expect(api.workspaces.sync).not.toHaveBeenCalled()
  expect(api.workspaces.remove).not.toHaveBeenCalled()
})
