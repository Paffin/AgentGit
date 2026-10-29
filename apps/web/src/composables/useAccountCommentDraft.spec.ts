import { effectScope, reactive, ref } from 'vue'
import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { useAccountCommentDraft } from './useAccountCommentDraft'
import { ApiError } from '@/api/errors'
const api = vi.hoisted(() => ({
  commentDraft: vi.fn(),
  saveCommentDraft: vi.fn(),
  clearCommentDraft: vi.fn(),
}))
vi.mock('@/api/instance', () => ({ api: { proposals: api } }))
const auth = reactive({
  identity: 'user:alice',
  token: 'token',
  status: 'ready',
  capabilities: { features: ['proposals.comment_drafts'] },
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const empty = { present: false, version: 0, head: '', body: '', updated_at_ms: 0 }
const saved = { present: true, version: 2, head: 'old', body: 'Account notes', updated_at_ms: 1 }
const scopes: ReturnType<typeof effectScope>[] = []
afterEach(() => scopes.splice(0).forEach((s) => s.stop()))
beforeEach(() => {
  auth.identity = 'user:alice'
  auth.token = 'token'
  auth.capabilities.features = ['proposals.comment_drafts']
  api.commentDraft.mockReset().mockResolvedValue(empty)
  api.saveCommentDraft.mockReset().mockResolvedValue(saved)
  api.clearCommentDraft.mockReset().mockResolvedValue({ ...empty, version: 3 })
})
function show(body = '') {
  const scope = effectScope()
  scopes.push(scope)
  const local = { body: ref(body), head: ref('current') }
  const draft = scope.run(() =>
    useAccountCommentDraft({ repo: () => 'r', proposal: () => 'p', open: () => true, local }),
  )!
  return { draft, local }
}
it('restores only empty local content and keeps the inspected head', async () => {
  api.commentDraft.mockResolvedValue(saved)
  const { local } = show()
  await flushPromises()
  expect(local.body.value).toBe(saved.body)
  expect(local.head.value).toBe('old')
  expect(api.saveCommentDraft).not.toHaveBeenCalled()
})
it('keeps differing local content until explicitly adopting the account draft', async () => {
  api.commentDraft.mockResolvedValue(saved)
  const { draft, local } = show('Local')
  await flushPromises()
  expect(local.body.value).toBe('Local')
  expect(draft.canSave.value).toBe(false)
  draft.useAccount()
  expect(local.body.value).toBe(saved.body)
  expect(draft.cleanupVersion()).toBe(2)
})
it('rejects oversized UTF8 notes and uses the fetched version for explicit saves', async () => {
  const { draft, local } = show('Notes')
  await flushPromises()
  local.body.value = 'я'.repeat(8193)
  expect(await draft.save()).toBe(false)
  expect(api.saveCommentDraft).not.toHaveBeenCalled()
  local.body.value = 'Notes'
  expect(await draft.save()).toBe(true)
  expect(api.saveCommentDraft).toHaveBeenCalledWith(
    'r',
    'p',
    { head: 'current', body: 'Notes', expected_version: 0 },
    expect.objectContaining({ token: 'token' }),
  )
})
it('ignores a load completing after account change', async () => {
  let finish!: (v: typeof saved) => void
  api.commentDraft.mockImplementationOnce(() => new Promise((r) => (finish = r)))
  const { local } = show()
  auth.identity = 'user:other'
  auth.token = 'other'
  await flushPromises()
  finish(saved)
  await flushPromises()
  expect(local.body.value).toBe('')
})
it('never clears a different account draft and treats cleanup conflicts as a warning', async () => {
  api.commentDraft.mockResolvedValue(saved)
  const { draft } = show('Local')
  await flushPromises()
  draft.keepLocal()
  expect(draft.cleanupVersion()).toBe(null)
  expect(await draft.clearAfterSubmit()).toBe('')
  expect(api.clearCommentDraft).not.toHaveBeenCalled()
  await draft.load()
  draft.useAccount()
  api.clearCommentDraft.mockRejectedValue(new Error('Newer draft'))
  expect(await draft.clearAfterSubmit()).toContain('Newer draft')
})
it('does not call account APIs on older servers', async () => {
  auth.capabilities.features = []
  const { draft } = show()
  await flushPromises()
  expect(draft.supported.value).toBe(false)
  expect(api.commentDraft).not.toHaveBeenCalled()
})
it('preserves text typed while the account load is pending', async () => {
  let finish!: (v: typeof saved) => void
  api.commentDraft.mockImplementationOnce(() => new Promise((r) => (finish = r)))
  const { local, draft } = show()
  local.body.value = 'Typed while loading'
  finish(saved)
  await flushPromises()
  expect(local.body.value).toBe('Typed while loading')
  expect(draft.available.value).toEqual(saved)
})
it('cleans up the submitted version even if later local work changes the account version', async () => {
  api.commentDraft.mockResolvedValue(saved)
  const { draft } = show()
  await flushPromises()
  const submittedVersion = draft.cleanupVersion()
  api.saveCommentDraft.mockResolvedValue({ ...saved, version: 3 })
  await draft.save()
  await draft.clearAfterSubmit(submittedVersion)
  expect(api.clearCommentDraft).toHaveBeenCalledWith(
    'r',
    'p',
    2,
    expect.objectContaining({ token: 'token' }),
  )
})

it('accepts exactly 16 KiB of UTF8 text', async () => {
  const { draft } = show('я'.repeat(8192))
  await flushPromises()
  expect(await draft.save()).toBe(true)
  expect(api.saveCommentDraft).toHaveBeenCalledTimes(1)
})
it('requires reload after a conflicting save without discarding local work', async () => {
  const { draft, local } = show('Keep my comment')
  await flushPromises()
  api.saveCommentDraft.mockRejectedValue(
    new ApiError({
      status: 412,
      code: 'failed_precondition',
      message: 'Another device saved a draft',
    }),
  )
  expect(await draft.save()).toBe(false)
  expect(draft.canSave.value).toBe(false)
  expect(local.body.value).toBe('Keep my comment')
  api.commentDraft.mockResolvedValue(saved)
  await draft.load()
  expect(draft.available.value?.body).toBe(saved.body)
  draft.keepLocal()
  expect(draft.canSave.value).toBe(true)
})
