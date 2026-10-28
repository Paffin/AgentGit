import { afterEach, expect, it } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { enableAutoUnmount, mount } from '@vue/test-utils'
import { useCommentDraft } from './useCommentDraft'
enableAutoUnmount(afterEach)

function host(scope: () => string, head: () => string) {
  let draft!: ReturnType<typeof useCommentDraft>
  const component = mount(
    defineComponent({
      setup() {
        draft = useCommentDraft(scope, head)
        return () => h('div')
      },
    }),
  )
  return { draft, component }
}

it('restores the text, inspected head and retry key after remount', () => {
  sessionStorage.clear()
  const first = host(
    () => 'alice/repo/proposal',
    () => 'head-one',
  )
  first.draft.body.value = 'Why this implementation?'
  const key = first.draft.requestId.value
  first.component.unmount()
  const restored = host(
    () => 'alice/repo/proposal',
    () => 'head-two',
  ).draft
  expect(restored.body.value).toBe('Why this implementation?')
  expect(restored.head.value).toBe('head-one')
  expect(restored.requestId.value).toBe(key)
  restored.head.value = 'head-two'
  expect(restored.requestId.value).not.toBe(key)
  restored.clear()
  expect(sessionStorage.getItem('agentgit.proposal-comment.v1:alice/repo/proposal')).toBeNull()
})

it('separates accounts synchronously and leaves a malformed saved draft untouched', () => {
  sessionStorage.clear()
  const account = ref('alice')
  const draft = host(
    () => account.value,
    () => 'head',
  ).draft
  draft.body.value = 'Private draft'
  sessionStorage.setItem('agentgit.proposal-comment.v1:bob', 'corrupt')
  account.value = 'bob'
  expect(draft.body.value).toBe('')
  expect(draft.error.value).toContain('left untouched')
  draft.body.value = 'New text'
  expect(sessionStorage.getItem('agentgit.proposal-comment.v1:bob')).toBe('corrupt')
  account.value = 'alice'
  expect(draft.body.value).toBe('Private draft')
})
