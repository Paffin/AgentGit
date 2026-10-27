import { describe, expect, it } from 'vitest'
import { defineComponent, h, nextTick, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { useUnsavedChanges } from './useUnsavedChanges'

async function editor() {
  const dirty = ref(false)
  const busy = ref(false)
  let guard!: ReturnType<typeof useUnsavedChanges>
  const Edit = defineComponent({
    setup() {
      guard = useUnsavedChanges(dirty, busy)
      return () => h('textarea')
    },
  })
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/edit', component: Edit },
      { path: '/away', component: { render: () => h('p', 'away') } },
    ],
  })
  await router.push('/edit')
  const wrapper = mount({ render: () => h(RouterView) }, { global: { plugins: [router] } })
  await nextTick()
  return { dirty, busy, router, wrapper, guard }
}

function unload() {
  const event = new Event('beforeunload', { cancelable: true })
  window.dispatchEvent(event)
  return event.defaultPrevented
}

describe('unfinished work', () => {
  it('protects tab closes only while there is work or a request in flight', async () => {
    const e = await editor()
    expect(unload()).toBe(false)
    e.dirty.value = true
    await nextTick()
    expect(unload()).toBe(true)
    e.dirty.value = false
    e.busy.value = true
    await nextTick()
    expect(unload()).toBe(true)
    e.wrapper.unmount()
    expect(unload()).toBe(false)
  })

  it('keeps work after refusing navigation, then leaves only after explicit discard', async () => {
    const e = await editor()
    e.dirty.value = true
    const denied = e.router.push('/away')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(e.guard.askingToLeave.value).toBe(true)
    e.guard.decide(false)
    await denied
    expect(e.router.currentRoute.value.path).toBe('/edit')
    const allowed = e.router.push('/away')
    await new Promise((resolve) => setTimeout(resolve, 0))
    e.guard.decide(true)
    await allowed
    expect(e.router.currentRoute.value.path).toBe('/away')
    e.wrapper.unmount()
  })

  it('allows harmless anchors but protects revision changes and blocks leaving during publication', async () => {
    const e = await editor()
    e.dirty.value = true
    await e.router.push('/edit#details')
    expect(e.guard.askingToLeave.value).toBe(false)
    const change = e.router.push('/edit?rev=other')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(e.guard.askingToLeave.value).toBe(true)
    e.guard.decide(false)
    await change
    expect(e.router.currentRoute.value.query.rev).toBeUndefined()
    e.busy.value = true
    await e.router.push('/away')
    expect(e.router.currentRoute.value.path).toBe('/edit')
    e.wrapper.unmount()
  })
})
