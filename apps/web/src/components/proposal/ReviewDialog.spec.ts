import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Proposal } from '@/api/types'
import ReviewDialog from './ReviewDialog.vue'

const { review, capabilities } = vi.hoisted(() => ({ review: vi.fn(), capabilities: vi.fn() }))
vi.mock('@/api/instance', () => ({ api: { capabilities, proposals: { review } } }))

const proposal: Proposal = {
  id: 'proposal-one',
  head: 'head-one',
  author: 'reviewer',
  created_at_ms: 0,
  updated_at_ms: 0,
  description: '',
  integrated_as: '',
  state: 'open',
  stream: 'feature',
  task_id: '',
  title: 'Fix timeout handling',
}

function show() {
  return mount(ReviewDialog, {
    props: { modelValue: true, repo: 'acme/demo', proposal },
    global: {
      plugins: [createPinia()],
      stubs: { Dialog: { template: '<div><slot/><slot name="footer"/></div>' } },
    },
  })
}

beforeEach(() => {
  review.mockReset()
  capabilities.mockReset().mockResolvedValue({ features: ['proposals.review_head'] })
})

describe('unfinished review', () => {
  it('keeps the comment and decision when closed to inspect the diff', async () => {
    const w = show()
    await w.get('textarea').setValue('Please handle the timeout.\nI checked the retry path.')
    await w.get('input[value="request_changes"]').setValue()
    await w.get('button').trigger('click')
    expect(w.emitted('update:modelValue')).toEqual([[false]])
    await w.setProps({ modelValue: false })
    await w.setProps({ modelValue: true })
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe(
      'Please handle the timeout.\nI checked the retry path.',
    )
    expect((w.get('input[value="request_changes"]').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('preserves a failed submission, clearing only after the server accepts the retry', async () => {
    review.mockRejectedValueOnce(new Error('Connection lost')).mockResolvedValueOnce({})
    const w = show()
    await w.get('textarea').setValue('Checked the retry path.')
    await w.get('input[value="comment"]').setValue()
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(w.text()).toContain('Connection lost')
    expect(w.emitted('done')).toBeUndefined()
    await w.setProps({ modelValue: false })
    await w.setProps({ modelValue: true })
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe('Checked the retry path.')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(review).toHaveBeenLastCalledWith('acme/demo', 'proposal-one', {
      decision: 'comment',
      expected_head: 'head-one',
      body: 'Checked the retry path.',
    })
    expect(w.emitted('done')).toEqual([[]])
    await w.setProps({ modelValue: false })
    await w.setProps({ modelValue: true })
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe('')
    expect((w.get('input[value="approve"]').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('keeps the draft and refuses servers without atomic review-head support', async () => {
    capabilities.mockResolvedValueOnce({ features: [] })
    const w = show()
    await w.get('textarea').setValue('A review must name the inspected head')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(review).not.toHaveBeenCalled()
    expect(w.text()).toContain('Update the server before submitting reviews')
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe(
      'A review must name the inspected head',
    )
    w.unmount()
  })

  it('keeps the draft when the same proposal is refreshed', async () => {
    const w = show()
    await w.get('textarea').setValue('Still reviewing this head')
    await w.setProps({ proposal: { ...proposal, title: 'Updated title' } })
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe(
      'Still reviewing this head',
    )
    expect(w.text()).not.toContain('The proposal changed')
    w.unmount()
  })

  it('does not send a second review while the first is pending', async () => {
    let finish!: () => void
    review.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    const w = show()
    await w.get('form').trigger('submit')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(review).toHaveBeenCalledTimes(1)
    finish()
    await flushPromises()
    w.unmount()
  })

  it('keeps the draft but requires re-review when the head changes', async () => {
    const w = show()
    await w.get('textarea').setValue('Checked the old implementation')
    await w.get('input[value="request_changes"]').setValue()
    await w.setProps({ proposal: { ...proposal, head: 'head-two' } })
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe(
      'Checked the old implementation',
    )
    expect(w.text()).toContain('The proposal changed')
    expect(w.get('button[type="submit"]').attributes('aria-disabled')).toBe('true')
    await w.get('form').trigger('submit')
    expect(review).not.toHaveBeenCalled()
    await w.setProps({ modelValue: false })
    await w.setProps({ modelValue: true })
    expect(w.text()).toContain('The proposal changed')
    await w
      .findAll('button')
      .find((b) => b.text() === 'I reviewed the updated changes')!
      .trigger('click')
    expect(w.find('[role="alert"]').exists()).toBe(false)
    expect(w.get('button[type="submit"]').attributes('aria-disabled')).toBeUndefined()
    review.mockResolvedValueOnce({})
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(review).toHaveBeenCalledWith('acme/demo', 'proposal-one', {
      decision: 'request_changes',
      expected_head: 'head-two',
      body: 'Checked the old implementation',
    })
    w.unmount()
  })

  it.each([
    ['repository', { repo: 'other/demo' }],
    ['proposal', { proposal: { ...proposal, id: 'proposal-two' } }],
  ])('does not carry a review to a different %s', async (_name, props) => {
    const w = show()
    await w.get('textarea').setValue('Only reviewed head-one')
    await w.get('input[value="request_changes"]').setValue()
    await w.setProps(props)
    expect((w.get('textarea').element as HTMLTextAreaElement).value).toBe('')
    expect((w.get('input[value="approve"]').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })
})
