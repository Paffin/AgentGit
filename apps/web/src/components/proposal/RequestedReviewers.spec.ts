import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { reactive } from 'vue'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { Proposal } from '@/api/types'
import RequestedReviewers from './RequestedReviewers.vue'

enableAutoUnmount(afterEach)
const setReviewers = vi.hoisted(() => vi.fn())
vi.mock('@/api/instance', () => ({ api: { proposals: { setReviewers } } }))
const auth = reactive({
  token: 'original',
  identity: 'user:alice',
  capabilities: { features: ['proposals.review_requests'] },
  can: () => true,
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
const proposal: Proposal = {
  id: 'p1',
  stream: 'main',
  head: 'h1',
  title: 'Change',
  description: '',
  author: 'user:alice',
  state: 'open',
  task_id: '',
  created_at_ms: 1,
  updated_at_ms: 1,
  integrated_as: '',
}
function show() {
  return mount(RequestedReviewers, {
    props: { repo: 'acme/app', proposal, reviewers: ['user:bob'], reviews: [] },
    global: { stubs: { Teleport: true } },
  })
}
beforeEach(() => {
  auth.identity = 'user:alice'
  auth.token = 'original'
  auth.capabilities.features = ['proposals.review_requests']
  setReviewers.mockReset().mockResolvedValue({ reviewers: ['user:carol'] })
})
it('saves the requested list with its inspected set only after confirmation', async () => {
  const wrapper = show()
  await wrapper.get('button').trigger('click')
  await wrapper.get('textarea').setValue('user:carol')
  expect(setReviewers).not.toHaveBeenCalled()
  await wrapper.get('form').trigger('submit')
  await flushPromises()
  expect(setReviewers).toHaveBeenCalledWith(
    'acme/app',
    'p1',
    ['user:carol'],
    ['user:bob'],
    expect.objectContaining({ token: 'original' }),
  )
  expect(wrapper.emitted('saved')).toBeTruthy()
})
it('retains a draft when a concurrent edit is refused', async () => {
  setReviewers.mockRejectedValue(new Error('requested reviewers changed'))
  const wrapper = show()
  await wrapper.get('button').trigger('click')
  await wrapper.get('textarea').setValue('user:carol')
  await wrapper.setProps({ reviewers: ['user:dave'] })
  await wrapper.get('form').trigger('submit')
  await flushPromises()
  expect(wrapper.get('textarea').element.value).toBe('user:carol')
  expect(wrapper.text()).toContain('requested reviewers changed')
  expect(setReviewers.mock.calls[0]![3]).toEqual(['user:bob'])
})
it('shows requests without editing controls to another user and hides unsupported controls', async () => {
  auth.identity = 'user:bob'
  const wrapper = show()
  expect(wrapper.text()).toContain('bob')
  expect(wrapper.find('button').exists()).toBe(false)
  auth.capabilities.features = []
  await flushPromises()
  expect(wrapper.find('section').exists()).toBe(false)
})

it('counts only review decisions on the current revision', async () => {
  const wrapper = show()
  const review = {
    id: 1,
    proposal: 'p1',
    reviewer: 'user:bob',
    decision: 'approve',
    body: '',
    head: 'old',
    timestamp_ms: 1,
  }
  await wrapper.setProps({ reviews: [review] })
  expect(wrapper.text()).toContain('Review requested')
  await wrapper.setProps({ reviews: [{ ...review, head: 'h1', decision: 'comment' }] })
  expect(wrapper.text()).toContain('Review requested')
  await wrapper.setProps({ reviews: [{ ...review, head: 'h1', decision: 'request_changes' }] })
  expect(wrapper.text()).toContain('Reviewed this revision')
})

it('rejects an oversized list and closes the draft when the session changes', async () => {
  const wrapper = show()
  await wrapper.get('button').trigger('click')
  await wrapper
    .get('textarea')
    .setValue(Array.from({ length: 21 }, (_, i) => `user:reviewer-${i}`).join('\n'))
  await wrapper.get('form').trigger('submit')
  expect(setReviewers).not.toHaveBeenCalled()
  expect(wrapper.text()).toContain('Request up to 20 reviewers')
  auth.token = 'another-session'
  await flushPromises()
  expect(wrapper.find('form').exists()).toBe(false)
})
