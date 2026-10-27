import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import FileUpload from './FileUpload.vue'

enableAutoUnmount(afterEach)
const { head, resolve } = vi.hoisted(() => ({ head: vi.fn(), resolve: vi.fn() }))
vi.mock('@/api/instance', () => ({ api: { streams: { head }, tree: { resolve } } }))
beforeEach(() => {
  head.mockReset().mockResolvedValue({ head: 'base' })
  resolve.mockReset().mockResolvedValue({ kind: 'missing' })
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))
vi.mock('@/stores/board', () => ({ useBoard: () => ({ touched: vi.fn() }) }))
vi.mock('@/stores/toasts', () => ({ useToasts: () => ({ ok: vi.fn() }) }))
vi.mock('@/composables/useUnsavedChanges', () => ({
  useUnsavedChanges: () => ({
    askingToLeave: ref(false),
    requestLeave: () => true,
    decide: vi.fn(),
  }),
}))

describe('upload selection', () => {
  it('retries the branch lookup without losing the upload or sending duplicate requests', async () => {
    head.mockRejectedValueOnce(new Error('Branch unavailable'))
    const w = mount(FileUpload, {
      props: { repo: 'acme/demo', stream: 'main', path: '' },
      global: { stubs: { Dialog: true } },
    })
    const file = new File(['contents'], 'one.txt')
    const input = w.get('input[type=file]')
    Object.defineProperty(input.element, 'files', { value: [file], configurable: true })
    await input.trigger('change')
    await w.findAll('input')[1]!.setValue('docs')
    await w.findAll('input')[2]!.setValue('Publish documentation')
    await w.get('textarea').setValue('Keep this description')
    await flushPromises()
    expect(w.text()).toContain('Branch unavailable')
    let finish!: (value: { head: string }) => void
    head.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const retry = w.findAll('button').find((button) => button.text().includes('Retry branch'))!
    expect(retry).toBeDefined()
    await retry.trigger('click')
    expect(retry.attributes('aria-disabled')).toBe('true')
    await retry.trigger('click')
    expect(head).toHaveBeenCalledTimes(2)
    expect(resolve).not.toHaveBeenCalled()
    expect(w.findAll('input')[1]!.element.value).toBe('docs')
    expect(w.findAll('input')[2]!.element.value).toBe('Publish documentation')
    expect(w.get('textarea').element.value).toBe('Keep this description')
    expect(w.text()).toContain('docs/one.txt')
    finish({ head: 'recovered-head' })
    await flushPromises()
    expect(w.text()).not.toContain('Branch unavailable')
    await w.get('form').trigger('submit')
    await flushPromises()
    expect(resolve).toHaveBeenCalledWith('acme/demo', 'recovered-head', 'docs/one.txt')
    expect(w.text()).toContain('Review before opening the change request')
  })

  it('keeps the suggested title in sync while preserving a title the user wrote', async () => {
    const w = mount(FileUpload, {
      props: { repo: 'acme/demo', stream: 'main', path: '' },
      global: { stubs: { Dialog: true } },
    })
    const input = w.get('input[type=file]')
    const files = [new File(['a'], 'one.txt'), new File(['b'], 'two.txt')]
    Object.defineProperty(input.element, 'files', { value: files, configurable: true })
    await input.trigger('change')
    await flushPromises()
    const title = w.findAll('input').find((field) => field.attributes('required') !== undefined)!
    expect((title.element as HTMLInputElement).value).toBe('Upload 2 files')
    await w
      .findAll('button')
      .find((button) => button.attributes('aria-label') === 'Remove one.txt')!
      .trigger('click')
    expect((title.element as HTMLInputElement).value).toBe('Upload 1 file')
    await title.setValue('Publish the revised assets')
    Object.defineProperty(input.element, 'files', { value: files, configurable: true })
    await input.trigger('change')
    expect((title.element as HTMLInputElement).value).toBe('Publish the revised assets')
    w.unmount()
  })
})
