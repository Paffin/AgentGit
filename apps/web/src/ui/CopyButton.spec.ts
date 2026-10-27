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

function pendingClipboard() {
  const attempts: { resolve: () => void; reject: (error: Error) => void }[] = []
  const writeText = vi.fn(
    () =>
      new Promise<void>((resolve, reject) => {
        attempts.push({ resolve, reject })
      }),
  )
  useClipboard({ writeText })
  return { attempts, writeText }
}

describe('copy attempt lifetime', () => {
  it('does not let an earlier success replace a later failure', async () => {
    const { attempts } = pendingClipboard()
    fallback(() => false)
    const w = show()
    await w.get('button').trigger('click')
    await w.get('button').trigger('click')
    attempts[1]!.reject(new Error('Latest attempt denied'))
    await flushPromises()
    expect(w.get('[role="status"]').text()).toBe('Copy failed')
    attempts[0]!.resolve()
    await flushPromises()
    expect(w.get('[role="status"]').text()).toBe('Copy failed')
    w.unmount()
  })

  it('does not run an obsolete fallback after a newer successful copy', async () => {
    const { attempts } = pendingClipboard()
    fallback(() => true)
    const w = show()
    await w.get('button').trigger('click')
    await w.get('button').trigger('click')
    attempts[1]!.resolve()
    await flushPromises()
    attempts[0]!.reject(new Error('Old attempt denied'))
    await flushPromises()
    expect(document.execCommand).not.toHaveBeenCalled()
    expect(w.get('[role="status"]').text()).toBe('Copied to clipboard')
    w.unmount()
  })

  it('does not copy or report success for a value replaced during an attempt', async () => {
    const { attempts, writeText } = pendingClipboard()
    fallback(() => true)
    const w = show()
    await w.get('button').trigger('click')
    await w.setProps({ text: 'src/new-file.ts' })
    attempts[0]!.reject(new Error('Old value denied'))
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith('src/auth/session.ts')
    expect(document.execCommand).not.toHaveBeenCalled()
    expect(w.get('[role="status"]').text()).toBe('')
    await w.get('button').trigger('click')
    attempts[1]!.resolve()
    await flushPromises()
    expect(writeText).toHaveBeenLastCalledWith('src/new-file.ts')
    expect(w.get('[role="status"]').text()).toBe('Copied to clipboard')
    w.unmount()
  })

  it('clears old feedback as soon as the displayed value changes', async () => {
    const { attempts } = pendingClipboard()
    const w = show()
    await w.get('button').trigger('click')
    attempts[0]!.resolve()
    await flushPromises()
    expect(w.get('[role="status"]').text()).toBe('Copied to clipboard')
    await w.setProps({ text: 'new value' })
    expect(w.get('[role="status"]').text()).toBe('')
    w.unmount()
  })

  it('does not fall back or schedule feedback after unmounting', async () => {
    vi.useFakeTimers()
    const { attempts } = pendingClipboard()
    fallback(() => true)
    const w = show()
    await w.get('button').trigger('click')
    w.unmount()
    await flushPromises()
    const timersAfterUnmount = vi.getTimerCount()
    attempts[0]!.reject(new Error('Arrived after unmount'))
    await flushPromises()
    expect(document.execCommand).not.toHaveBeenCalled()
    expect(document.querySelector('textarea')).toBeNull()
    expect(vi.getTimerCount()).toBe(timersAfterUnmount)
  })
})
