import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { Proposal } from '@/api/types'
import { ApiError } from '@/api/errors'
import ConflictResolver from './ConflictResolver.vue'

enableAutoUnmount(afterEach)
const api = vi.hoisted(() => ({
  workspaces: { create: vi.fn(), sync: vi.fn(), remove: vi.fn() },
  tree: { resolve: vi.fn(), file: vi.fn(), list: vi.fn() },
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
  auth.capabilities.features = [
    'proposals.update_head',
    'workspaces.sync_precondition',
    'workspaces.conflict_origins',
  ]
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

it('loads renamed content from each source path and writes the selected bytes at the output path', async () => {
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [
        {
          path: 'renamed.txt',
          kind: 'content',
          origin: { base: 'old.txt', ours: 'renamed.txt', theirs: 'old.txt' },
        },
      ],
    })
    .mockResolvedValue({ status: 'merged', head: 'merged-head' })
  api.tree.resolve.mockImplementation(async (_repo, head, path) => ({
    kind: 'regular',
    entry: {
      path,
      blob_id: head === 'mine-head' ? 'mine-blob' : 'branch-blob',
      file_type: 'regular',
      executable: false,
      size: 12,
    },
  }))
  api.tree.file.mockImplementation(async (_repo, head) => ({
    bytes: new TextEncoder().encode(
      head === 'mine-head' ? 'my renamed edit' : 'branch original edit',
    ),
    truncated: false,
  }))
  const w = show()
  await flushPromises()
  expect(api.tree.resolve).toHaveBeenCalledWith(
    'acme/app',
    'branch-head',
    'old.txt',
    expect.anything(),
  )
  expect(w.text()).toContain('Source: old.txt')
  expect(w.text()).toContain('branch original edit')
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep the branch’s')!
    .trigger('click')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(api.workspaces.sync.mock.calls[1]![2]).toEqual([
    {
      path: 'renamed.txt',
      op: 'put',
      blob_id: 'branch-blob',
      file_type: 'regular',
      executable: false,
    },
  ])
})

it('refuses servers without source-path metadata before preparing a merge', async () => {
  auth.capabilities.features = ['proposals.update_head', 'workspaces.sync_precondition']
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Upgrade the server')
  expect(api.workspaces.create).not.toHaveBeenCalled()
})

it.each(['mine', 'branch'])(
  'handles rename/delete with the %s side missing at the output path',
  async (missing) => {
    api.workspaces.sync
      .mockReset()
      .mockResolvedValueOnce({
        status: 'conflicted',
        stream_head: 'branch-head',
        conflicts: [{ path: 'renamed.txt', kind: 'rename_delete' }],
      })
      .mockResolvedValue({ status: 'merged', head: 'merged-head' })
    api.tree.resolve.mockImplementation(async (_repo, head, path) =>
      head === `${missing}-head`
        ? { kind: 'missing', entry: null }
        : {
            kind: 'regular',
            entry: { path, blob_id: 'kept-blob', file_type: 'regular', executable: true, size: 3 },
          },
    )
    const w = show()
    await flushPromises()
    expect(w.text()).not.toContain('Not here')
    expect(w.text()).toContain(
      missing === 'mine' ? '(you deleted this file)' : '(the branch deleted this file)',
    )
    const keep = missing === 'mine' ? 'Keep the branch’s' : 'Keep mine'
    await w
      .findAll('button')
      .find((b) => b.text() === keep)!
      .trigger('click')
    await w.get('[data-testid=resolve-save]').trigger('click')
    await flushPromises()
    expect(api.workspaces.sync.mock.calls[1]![2]).toEqual([
      {
        path: 'renamed.txt',
        op: 'put',
        blob_id: 'kept-blob',
        file_type: 'regular',
        executable: true,
      },
    ])
  },
)

it.each(['rename_rename', 'file_directory'])(
  'keeps %s structural conflicts blocked',
  async (kind) => {
    api.workspaces.sync.mockReset().mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [{ path: 'renamed.txt', kind }],
    })
    const w = show()
    await flushPromises()
    expect(w.text()).toContain('Not here')
    expect(api.tree.resolve).not.toHaveBeenCalled()
    expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
  },
)

it('groups nested structural conflicts and emits exact whole-folder puts and synthetic deletes', async () => {
  auth.capabilities.features.push('workspaces.structural_preview')
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [
        {
          path: 'a',
          kind: 'file_directory',
          resolution_scope: { complete: true, paths: ['a', 'a/synthetic', 'a/run'] },
        },
        { path: 'a/run', kind: 'content' },
      ],
    })
    .mockResolvedValue({ status: 'merged', head: 'merged-head' })
  api.tree.resolve.mockImplementation(async (_repo, head, path) =>
    head === 'mine-head'
      ? {
          kind: 'regular',
          entry: { path, blob_id: 'file-blob', file_type: 'regular', executable: false, size: 3 },
        }
      : { kind: 'dir', entry: null },
  )
  api.tree.list = vi.fn().mockResolvedValue({
    entries: [{ name: 'run', kind: 'file', blob_id: 'run-blob', executable: true, size: 3 }],
    nextPageToken: '',
  })
  const w = show()
  await flushPromises()
  expect(w.findAll('section.row')).toHaveLength(1)
  expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep the branch’s (1 file)')!
    .trigger('click')
  expect(w.text()).toContain('a/synthetic')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(api.workspaces.sync.mock.calls[1]![2]).toEqual([
    { path: 'a', op: 'delete' },
    { path: 'a/run', op: 'put', blob_id: 'run-blob', file_type: 'regular', executable: true },
    { path: 'a/synthetic', op: 'delete' },
  ])
})
it.each([
  undefined,
  { complete: false, paths: ['a'] },
  { complete: true, paths: ['a', 'outside'] },
  { complete: true, paths: ['a/child'] },
])('blocks incomplete or invalid structural preview %j', async (scope) => {
  auth.capabilities.features.push('workspaces.structural_preview')
  api.workspaces.sync.mockReset().mockResolvedValue({
    status: 'conflicted',
    stream_head: 'branch-head',
    conflicts: [{ path: 'a', kind: 'file_directory', resolution_scope: scope }],
  })
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Not here')
  expect(w.get('[data-testid=resolve-save]').attributes('aria-disabled')).toBe('true')
  expect(api.tree.resolve).not.toHaveBeenCalled()
})

it('requires structural capability even when a complete scope is present', async () => {
  api.workspaces.sync.mockReset().mockResolvedValue({
    status: 'conflicted',
    stream_head: 'branch-head',
    conflicts: [
      { path: 'a', kind: 'file_directory', resolution_scope: { complete: true, paths: ['a'] } },
    ],
  })
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('Upgrade the server')
  expect(api.tree.resolve).not.toHaveBeenCalled()
})
it('makes an absent source an explicit removal including synthetic candidate descendants', async () => {
  auth.capabilities.features.push('workspaces.structural_preview')
  api.workspaces.sync
    .mockReset()
    .mockResolvedValueOnce({
      status: 'conflicted',
      stream_head: 'branch-head',
      conflicts: [
        {
          path: 'a',
          kind: 'file_directory',
          resolution_scope: { complete: true, paths: ['a', 'a/synthetic'] },
        },
      ],
    })
    .mockResolvedValue({ status: 'merged', head: 'merged-head' })
  api.tree.resolve.mockResolvedValue({ kind: 'missing', entry: null })
  const w = show()
  await flushPromises()
  await w
    .findAll('button')
    .find((b) => b.text() === 'Remove path (absent in mine)')!
    .trigger('click')
  expect(w.text()).toContain('No file or folder at this path')
  expect(w.text()).toContain('a/synthetic')
  await w.get('[data-testid=resolve-save]').trigger('click')
  await flushPromises()
  expect(api.workspaces.sync.mock.calls[1]![2]).toEqual([
    { path: 'a', op: 'delete' },
    { path: 'a/synthetic', op: 'delete' },
  ])
})
