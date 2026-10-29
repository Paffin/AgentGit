import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import ProposalComment from './ProposalComment.vue'
import type { Proposal } from '@/api/types'
enableAutoUnmount(afterEach)
const api = vi.hoisted(() => ({
  comment: vi.fn(),
  commentDraft: vi.fn(),
  saveCommentDraft: vi.fn(),
  clearCommentDraft: vi.fn(),
}))
vi.mock('@/api/instance', () => ({ api: { proposals: api } }))
const toasts = vi.hoisted(() => ({ ok: vi.fn(), info: vi.fn(), warn: vi.fn() }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => toasts }))
const auth = reactive({
  identity: 'user:alice',
  token: 'token',
  status: 'ready',
  can: () => true,
  capabilities: { features: ['proposals.comments', 'proposals.comment_drafts'] },
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const empty = { present: false, version: 0, head: '', body: '', updated_at_ms: 0 }
const saved = { present: true, version: 2, head: 'old', body: 'Account comment', updated_at_ms: 1 }
const proposal = { id: 'p', head: 'current' } as Proposal
const show = () => mount(ProposalComment, { props: { repo: 'r', proposal } })
beforeEach(() => {
  sessionStorage.clear()
  auth.identity = 'user:alice'
  auth.token = 'token'
  auth.capabilities.features = ['proposals.comments', 'proposals.comment_drafts']
  api.commentDraft.mockReset().mockResolvedValue(empty)
  api.saveCommentDraft.mockReset().mockResolvedValue(saved)
  api.clearCommentDraft.mockReset().mockResolvedValue(empty)
  api.comment.mockReset().mockResolvedValue({})
  Object.values(toasts).forEach((mock) => mock.mockClear())
})
it('explicitly saves the account draft without publishing or clearing local text', async () => {
  const w = show()
  await flushPromises()
  await w.get('textarea').setValue('Draft')
  await w
    .findAll('button')
    .find((b) => b.text() === 'Save draft')!
    .trigger('click')
  await flushPromises()
  expect(api.saveCommentDraft).toHaveBeenCalledWith(
    'r',
    'p',
    { body: 'Draft', head: 'current', expected_version: 0 },
    expect.objectContaining({ token: 'token' }),
  )
  expect(api.comment).not.toHaveBeenCalled()
  expect(w.get('textarea').element.value).toBe('Draft')
})
it('restores account text but requires reinspection of an old head before posting', async () => {
  api.commentDraft.mockResolvedValue(saved)
  const w = show()
  await flushPromises()
  expect(w.get('textarea').element.value).toBe(saved.body)
  expect(w.text()).toContain('earlier head')
  expect(
    w
      .findAll('button')
      .find((b) => b.text() === 'Add comment')!
      .attributes('aria-disabled'),
  ).toBe('true')
})
it('successful publication stays successful if account cleanup encounters newer work', async () => {
  api.commentDraft.mockResolvedValue({ ...saved, head: 'current' })
  api.clearCommentDraft.mockRejectedValue(new Error('newer draft'))
  const w = show()
  await flushPromises()
  await w.trigger('submit')
  await flushPromises()
  expect(api.comment).toHaveBeenCalledTimes(1)
  expect(api.clearCommentDraft).toHaveBeenCalledWith(
    'r',
    'p',
    2,
    expect.objectContaining({ token: 'token' }),
  )
  expect(w.emitted('done')).toHaveLength(1)
  expect(w.get('textarea').element.value).toBe('')
  expect(toasts.warn).toHaveBeenCalled()
  expect(w.text()).not.toContain('newer draft')
})
it('keeps tab-only fallback on older servers', async () => {
  auth.capabilities.features = ['proposals.comments']
  const w = show()
  await flushPromises()
  expect(w.text()).not.toContain('Save draft')
  expect(api.commentDraft).not.toHaveBeenCalled()
})
it('keeps a different account draft when publishing the tab draft', async () => {
  const key = `agentgit.proposal-comment.v1:${JSON.stringify(['user:alice', 'r', 'p'])}`
  sessionStorage.setItem(
    key,
    JSON.stringify({ version: 1, body: 'Tab comment', head: 'current', requestId: 'request' }),
  )
  api.commentDraft.mockResolvedValue({ ...saved, head: 'current' })
  const w = show()
  await flushPromises()
  expect(w.text()).toContain('An account draft is available')
  expect(w.get('textarea').element.value).toBe('Tab comment')
  await w
    .findAll('button')
    .find((b) => b.text() === 'Keep tab draft')!
    .trigger('click')
  await w.trigger('submit')
  await flushPromises()
  expect(api.comment).toHaveBeenCalledTimes(1)
  expect(api.clearCommentDraft).not.toHaveBeenCalled()
})
it('ignores a post response after switching accounts without clearing the new account draft', async () => {
  let finish!: () => void
  api.comment.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve
      }),
  )
  api.commentDraft.mockResolvedValue({ ...saved, head: 'current' })
  const w = show()
  await flushPromises()
  await w.trigger('submit')
  expect(api.comment).toHaveBeenCalledWith(
    'r',
    'p',
    expect.anything(),
    expect.objectContaining({ token: 'token' }),
  )
  auth.identity = 'user:bob'
  auth.token = 'bob-token'
  await flushPromises()
  finish()
  await flushPromises()
  expect(api.clearCommentDraft).not.toHaveBeenCalled()
  expect(w.emitted('done')).toBeUndefined()
})
