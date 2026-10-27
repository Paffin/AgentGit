import { mount } from '@vue/test-utils'
import { defineComponent, h, ref } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useScrollRegion } from './useScrollRegion'

let width = 400
let contentWidth = 300
let sizes: SizeObserver[] = []
let mutations: ContentObserver[] = []
class SizeObserver {
  observe = vi.fn()
  unobserve = vi.fn()
  disconnect = vi.fn()
  constructor(readonly callback: ResizeObserverCallback) {
    sizes.push(this)
  }
  fire() {
    this.callback([], this as unknown as ResizeObserver)
  }
}
class ContentObserver {
  observe = vi.fn()
  disconnect = vi.fn()
  constructor(readonly callback: MutationCallback) {
    mutations.push(this)
  }
  fire() {
    this.callback([], this as unknown as MutationObserver)
  }
}
function host(initiallyVisible: boolean) {
  return defineComponent({
    setup() {
      const visible = ref(initiallyVisible)
      const identity = ref(0)
      const text = ref('Code')
      const region = useScrollRegion(() => 'Example diff')
      return () =>
        h('section', [
          h(
            'button',
            {
              id: 'toggle',
              onClick: () => {
                visible.value = !visible.value
              },
            },
            'Toggle',
          ),
          h(
            'button',
            {
              id: 'replace',
              onClick: () => {
                identity.value++
              },
            },
            'Replace',
          ),
          h(
            'button',
            {
              id: 'update',
              onClick: () => {
                text.value += '.'
              },
            },
            'Update',
          ),
          visible.value
            ? h(
                'div',
                {
                  key: identity.value,
                  ref: region.setEl,
                  'data-test': 'region',
                  ...region.attrs.value,
                },
                [h('span', text.value)],
              )
            : null,
        ])
    },
  })
}
beforeEach(() => {
  width = 400
  contentWidth = 300
  sizes = []
  mutations = []
  vi.stubGlobal('ResizeObserver', SizeObserver)
  vi.stubGlobal('MutationObserver', ContentObserver)
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(() => width)
  vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockImplementation(() => contentWidth)
})
afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('scroll region lifetime', () => {
  it('measures an initially mounted region and does not recreate observers on ordinary updates', async () => {
    contentWidth = 800
    const w = mount(host(true))
    await w.vm.$nextTick()
    expect(w.get('[data-test="region"]').attributes('tabindex')).toBe('0')
    expect(sizes).toHaveLength(1)
    expect(mutations).toHaveLength(1)
    await w.get('#update').trigger('click')
    expect(sizes).toHaveLength(1)
    expect(mutations).toHaveLength(1)
    w.unmount()
    expect(sizes[0]!.disconnect).toHaveBeenCalledTimes(1)
    expect(mutations[0]!.disconnect).toHaveBeenCalledTimes(1)
  })

  it('observes a late-mounted region and updates keyboard access when it resizes', async () => {
    const w = mount(host(false))
    expect(sizes).toHaveLength(0)
    await w.get('#toggle').trigger('click')
    const box = w.get('[data-test="region"]')
    expect(box.attributes('tabindex')).toBeUndefined()
    expect(sizes).toHaveLength(1)
    width = 200
    sizes[0]!.fire()
    await w.vm.$nextTick()
    expect(box.attributes('tabindex')).toBe('0')
    expect(box.attributes('role')).toBe('region')
    expect(box.attributes('aria-label')).toBe('Example diff (scrolls sideways)')
    width = 400
    sizes[0]!.fire()
    await w.vm.$nextTick()
    expect(box.attributes('tabindex')).toBeUndefined()
    w.unmount()
  })

  it('disconnects the replaced element and observes only the new element and its children', async () => {
    const w = mount(host(true))
    await w.vm.$nextTick()
    const previous = w.get('[data-test="region"]').element
    await w.get('#replace').trigger('click')
    const current = w.get('[data-test="region"]').element
    expect(current).not.toBe(previous)
    expect(sizes[0]!.disconnect).toHaveBeenCalledTimes(1)
    expect(mutations[0]!.disconnect).toHaveBeenCalledTimes(1)
    expect(sizes).toHaveLength(2)
    expect(sizes[1]!.observe).toHaveBeenCalledWith(current)
    expect(sizes[1]!.observe).toHaveBeenCalledWith(current.firstElementChild)
    expect(mutations[1]!.observe).toHaveBeenCalledWith(current, expect.anything())
    contentWidth = 900
    mutations[1]!.fire()
    await w.vm.$nextTick()
    expect(w.get('[data-test="region"]').attributes('tabindex')).toBe('0')
    await w.get('#toggle').trigger('click')
    expect(sizes[1]!.disconnect).toHaveBeenCalledTimes(1)
    expect(mutations[1]!.disconnect).toHaveBeenCalledTimes(1)
    w.unmount()
    expect(sizes).toHaveLength(2)
  })

  it('keeps one font listener for the component lifetime and removes it on unmount', async () => {
    const descriptor = Object.getOwnPropertyDescriptor(document, 'fonts')
    const addEventListener = vi.fn()
    const removeEventListener = vi.fn()
    Object.defineProperty(document, 'fonts', {
      configurable: true,
      value: { addEventListener, removeEventListener },
    })
    const w = mount(host(true))
    try {
      await w.vm.$nextTick()
      await w.get('#toggle').trigger('click')
      await w.get('#toggle').trigger('click')
      expect(addEventListener).toHaveBeenCalledTimes(1)
      const listener = addEventListener.mock.calls[0]![1] as () => void
      contentWidth = 800
      listener()
      await w.vm.$nextTick()
      expect(w.get('[data-test="region"]').attributes('tabindex')).toBe('0')
      w.unmount()
      expect(removeEventListener).toHaveBeenCalledExactlyOnceWith('loadingdone', listener)
    } finally {
      if (descriptor) Object.defineProperty(document, 'fonts', descriptor)
      else Reflect.deleteProperty(document, 'fonts')
    }
  })
})
