import { effectScope, reactive, ref } from 'vue'
import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { useAccountReviewDraft } from './useAccountReviewDraft'

const api = vi.hoisted(() => ({
  reviewDraft: vi.fn(),
  saveReviewDraft: vi.fn(),
  clearReviewDraft: vi.fn(),
}))
vi.mock('@/api/instance', () => ({ api: { proposals: api } }))
const auth = reactive({
  identity: 'user:alice',
  token: 'first-token',
  status: 'ready',
  capabilities: { features: ['proposals.review_drafts'] },
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const empty = { present: false, version: 0, head: '', decision: '', body: '', updated_at_ms: 0 }
const saved = {
  present: true,
  version: 4,
  head: 'head-one',
  decision: 'request_changes',
  body: 'Handle the timeout.',
  updated_at_ms: 1,
}
const scopes: ReturnType<typeof effectScope>[] = []
afterEach(() => {
  scopes.splice(0).forEach((scope) => scope.stop())
})
beforeEach(() => {
  auth.identity = 'user:alice'
  auth.token = 'first-token'
  auth.status = 'ready'
  auth.capabilities.features = ['proposals.review_drafts']
  api.reviewDraft.mockReset().mockResolvedValue(empty)
  api.saveReviewDraft.mockReset().mockImplementation(async (_repo, _id, input) => ({
    ...input,
    present: true,
    version: 5,
    updated_at_ms: 2,
  }))
  api.clearReviewDraft.mockReset().mockResolvedValue({ ...empty, version: 5 })
})
function show(body = '', decision: 'approve' | 'request_changes' | 'comment' = 'approve') {
  const local = { head: ref('head-one'), body: ref(body), decision: ref(decision) }
  const open = ref(true)
  const scope = effectScope()
  scopes.push(scope)
  const draft = scope.run(() =>
    useAccountReviewDraft({
      repo: () => 'acme/app',
      proposal: () => 'proposal-one',
      open: () => open.value,
      local,
    }),
  )!
  return { draft, local, open }
}
it('restores an account draft into an empty local review without submitting it', async () => {
  api.reviewDraft.mockResolvedValue(saved)
  const { draft, local } = show()
  await flushPromises()
  expect(local.body.value).toBe(saved.body)
  expect(local.decision.value).toBe('request_changes')
  expect(local.head.value).toBe('head-one')
  expect(draft.version.value).toBe(4)
  expect(api.saveReviewDraft).not.toHaveBeenCalled()
})
it('keeps a differing local review until the user selects the account draft', async () => {
  api.reviewDraft.mockResolvedValue(saved)
  const { draft, local } = show('Local reasoning', 'comment')
  await flushPromises()
  expect(local.body.value).toBe('Local reasoning')
  expect(draft.available.value?.body).toBe(saved.body)
  expect(draft.canSave.value).toBe(false)
  draft.useAccount()
  expect(local.body.value).toBe(saved.body)
  expect(draft.canSave.value).toBe(true)
})
it('does not overwrite text typed while the account request is pending', async () => {
  let finish!: (value: typeof saved) => void
  api.reviewDraft.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const { draft, local } = show()
  local.body.value = 'Typed while loading'
  finish(saved)
  await flushPromises()
  expect(local.body.value).toBe('Typed while loading')
  expect(draft.available.value?.body).toBe(saved.body)
})
it('saves an inspected version with the original session and preserves a rejected draft', async () => {
  api.reviewDraft.mockResolvedValue({ ...empty, version: 7 })
  const { draft, local } = show('Local reasoning', 'comment')
  await flushPromises()
  api.saveReviewDraft.mockRejectedValue(new Error('draft changed'))
  expect(await draft.save()).toBe(false)
  expect(api.saveReviewDraft).toHaveBeenCalledWith(
    'acme/app',
    'proposal-one',
    { expected_version: 7, head: 'head-one', decision: 'comment', body: 'Local reasoning' },
    expect.objectContaining({ token: 'first-token' }),
  )
  expect(local.body.value).toBe('Local reasoning')
  expect(draft.error.value).toContain('draft changed')
})
it('does not apply a late response from another account', async () => {
  let finish!: (value: typeof saved) => void
  api.reviewDraft.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const { local } = show()
  auth.identity = 'user:other'
  auth.token = 'other-token'
  await flushPromises()
  finish(saved)
  await flushPromises()
  expect(local.body.value).toBe('')
})
it('reports cleanup conflicts separately from review submission', async () => {
  api.reviewDraft.mockResolvedValue(saved)
  const { draft } = show()
  await flushPromises()
  api.clearReviewDraft.mockRejectedValue(new Error('draft changed'))
  expect(await draft.clearAfterSubmit()).toContain('draft changed')
  expect(api.clearReviewDraft).toHaveBeenCalledWith(
    'acme/app',
    'proposal-one',
    4,
    expect.objectContaining({ token: 'first-token' }),
  )
})
it('does not call draft endpoints for older servers', async () => {
  auth.capabilities.features = []
  const { draft } = show()
  await flushPromises()
  expect(draft.supported.value).toBe(false)
  expect(api.reviewDraft).not.toHaveBeenCalled()
})
it('refuses more than 64 KiB before sending content', async () => {
  const { draft } = show('λ'.repeat(32769), 'comment')
  await flushPromises()
  expect(await draft.save()).toBe(false)
  expect(api.saveReviewDraft).not.toHaveBeenCalled()
  expect(draft.error.value).toContain('64 KiB')
})

it('keeps a different account draft when the local review is submitted', async () => {
  api.reviewDraft.mockResolvedValue(saved)
  const { draft } = show('Other local review', 'comment')
  await flushPromises()
  expect(draft.cleanupVersion()).toBeNull()
  expect(await draft.clearAfterSubmit()).toBe('')
  expect(api.clearReviewDraft).not.toHaveBeenCalled()
})
it('does not close or adopt a late save response after the account changes', async () => {
  let finish!: (value: typeof saved) => void
  api.saveReviewDraft.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const { draft } = show('Local review', 'comment')
  await flushPromises()
  const pending = draft.save()
  auth.identity = 'user:other'
  auth.token = 'other-token'
  await flushPromises()
  finish(saved)
  expect(await pending).toBe(false)
  expect(api.saveReviewDraft.mock.calls[0]![3].token).toBe('first-token')
})

it('clears the adopted draft after the reviewer explicitly inspects a newer head', async () => {
  api.reviewDraft.mockResolvedValue(saved)
  const { draft, local } = show()
  await flushPromises()
  local.head.value = 'newly-inspected-head'
  expect(draft.cleanupVersion()).toBe(4)
  await draft.clearAfterSubmit(draft.cleanupVersion())
  expect(api.clearReviewDraft.mock.calls[0]![2]).toBe(4)
})
