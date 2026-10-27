import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import FileUpload from './FileUpload.vue'

vi.mock('@/api/instance', () => ({ api: { streams: { head: async () => ({ head: 'base' }) } } }))
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
