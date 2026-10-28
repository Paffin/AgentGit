import { afterEach, expect, it } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { enableAutoUnmount, mount } from '@vue/test-utils'
import { useThreadAnchor } from './useThreadAnchor'
enableAutoUnmount(afterEach)
function host(scope: () => string) {
  let state!: ReturnType<typeof useThreadAnchor>
  const component = mount(
    defineComponent({
      setup() {
        state = useThreadAnchor(scope)
        return () => h('div')
      },
    }),
  )
  return { state, component }
}
it('restores the selected line while isolating accounts synchronously', () => {
  sessionStorage.clear()
  const scope = ref('alice:repo:proposal')
  const first = host(() => scope.value)
  first.state.anchor.value = {
    base: 'base',
    head: 'head',
    path: 'file.txt',
    side: 'new',
    line: 4,
    text: 'line',
  }
  scope.value = 'bob:repo:proposal'
  expect(first.state.anchor.value).toBeNull()
  scope.value = 'alice:repo:proposal'
  expect(first.state.anchor.value?.line).toBe(4)
  first.component.unmount()
  const next = host(() => scope.value).state
  expect(next.anchor.value?.path).toBe('file.txt')
  next.anchor.value = null
  expect(sessionStorage.getItem('agentgit.thread-anchor.v1:alice:repo:proposal')).toBeNull()
})
it('leaves malformed persisted input intact until a new line is explicitly selected', () => {
  sessionStorage.clear()
  const key = 'agentgit.thread-anchor.v1:bad'
  sessionStorage.setItem(key, '{broken')
  const state = host(() => 'bad').state
  expect(state.anchor.value).toBeNull()
  expect(state.error.value).toContain('could not be restored')
  expect(sessionStorage.getItem(key)).toBe('{broken')
})
