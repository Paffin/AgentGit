import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import CopyButton from './CopyButton.vue'

const clipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
const execCommand = Object.getOwnPropertyDescriptor(document, 'execCommand')

function useClipboard(value: unknown) {
  Object.defineProperty(navigator, 'clipboard', { value, configurable: true })
}
function fallback(implementation: () => boolean) {
  Object.defineProperty(document, 'execCommand', {
    value: vi.fn(implementation),
    configurable: true,
  })
  // Selecting a textarea also focuses it in the browser.
  vi.spyOn(HTMLTextAreaElement.prototype, 'select').mockImplementation(function (
    this: HTMLTextAreaElement,
  ) {
    this.focus()
  })
}
function show() {
  const w = mount(CopyButton, {
    props: { text: 'src/auth/session.ts', withLabel: true },
    attachTo: document.body,
  })
  w.get('button').element.focus()
  return w
}

beforeEach(() => useClipboard(undefined))
afterEach(() => {
  vi.restoreAllMocks()
  if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard)
  else Reflect.deleteProperty(navigator, 'clipboard')
  if (execCommand) Object.defineProperty(document, 'execCommand', execCommand)
  else Reflect.deleteProperty(document, 'execCommand')
})

describe('copy fallback', () => {
  it.each([true, false])(
    'restores the copy button after execCommand returns %s',
    async (success) => {
      fallback(() => success)
      const w = show()
      await w.get('button').trigger('click')
      expect(document.activeElement).toBe(w.get('button').element)
      expect(document.querySelector('textarea')).toBeNull()
      expect(w.get('[role="status"]').text()).toBe(success ? 'Copied to clipboard' : 'Copy failed')
      w.unmount()
    },
  )

  it('removes the temporary textarea and restores focus even if copying throws', async () => {
    fallback(() => {
      throw new Error('Clipboard blocked')
    })
    const w = show()
    await w.get('button').trigger('click')
    expect(document.querySelector('textarea')).toBeNull()
    expect(document.activeElement).toBe(w.get('button').element)
    expect(w.get('[role="status"]').text()).toBe('Copy failed')
    w.unmount()
  })

  it('does not restore focus to an element removed during copying', async () => {
    const previous = document.createElement('button')
    document.body.append(previous)
    fallback(() => {
      previous.remove()
      return true
    })
    const restore = vi.spyOn(previous, 'focus')
    const w = show()
    previous.focus()
    restore.mockClear()
    await w.get('button').trigger('click')
    expect(restore).not.toHaveBeenCalled()
    expect(document.querySelector('textarea')).toBeNull()
    w.unmount()
  })

  it('preserves the new focus when an async clipboard attempt fails later', async () => {
    let reject!: (reason: Error) => void
    useClipboard({
      writeText: () =>
        new Promise((_, fail) => {
          reject = fail
        }),
    })
    fallback(() => true)
    const w = show()
    await w.get('button').trigger('click')
    const next = document.createElement('input')
    document.body.append(next)
    next.focus()
    reject(new Error('Denied'))
    await flushPromises()
    expect(document.activeElement).toBe(next)
    expect(document.querySelector('textarea')).toBeNull()
    w.unmount()
  })

  it('reports a failed retry instead of keeping an earlier success', async () => {
    let success = true
    fallback(() => success)
    const w = show()
    await w.get('button').trigger('click')
    expect(w.get('[role="status"]').text()).toBe('Copied to clipboard')
    success = false
    await w.get('button').trigger('click')
    expect(w.get('[role="status"]').text()).toBe('Copy failed')
    expect(w.get('button').text()).toBe('Copy failed')
    w.unmount()
  })
})
