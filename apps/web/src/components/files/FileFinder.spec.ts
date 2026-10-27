import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/errors'
import FileFinder from './FileFinder.vue'

const mock = vi.hoisted(() => ({ resolve: vi.fn(), head: vi.fn(), manifest: vi.fn() }))
vi.mock('@/api/instance', () => ({
  api: {
    history: { resolve: mock.resolve },
    streams: { head: mock.head },
    tree: { manifest: mock.manifest },
  },
}))
const entry = (path: string) => ({
  path,
  blob_id: 'blob',
  file_type: 'regular',
  executable: false,
  size: 1,
})
const show = () =>
  mount(FileFinder, {
    attachTo: document.body,
    props: { modelValue: true, repo: 'acme/demo', rev: 'main' },
    global: { stubs: { Dialog: { template: '<div><slot/></div>' } } },
  })
beforeEach(() => {
  vi.resetAllMocks()
  mock.resolve.mockResolvedValue('fixed-revision')
})

describe('file finder', () => {
  it('searches full paths and loads later pages from one fixed revision', async () => {
    mock.manifest
      .mockResolvedValueOnce({ entries: [entry('src/app.ts')], nextPageToken: 'second', total: 2 })
      .mockResolvedValueOnce({ entries: [entry('deep/other.ts')], nextPageToken: '', total: 2 })
    const w = show()
    await flushPromises()
    await w.get('input').setValue('deep other')
    expect(w.findAll('a').map((a) => a.text())).toEqual(['deep/other.ts'])
    expect(mock.manifest).toHaveBeenLastCalledWith(
      'acme/demo',
      expect.objectContaining({ rev: 'fixed-revision', page_token: 'second' }),
      expect.anything(),
    )
    await w.get('a').trigger('click')
    expect(w.emitted('select')).toEqual([['deep/other.ts']])
    w.unmount()
  })
  it('shows an empty branch as empty instead of a failed search', async () => {
    mock.resolve.mockRejectedValue(
      new ApiError({ status: 404, code: 'not_found', message: 'no commits' }),
    )
    mock.head.mockResolvedValue({ head: '' })
    const w = show()
    await flushPromises()
    expect(w.text()).toContain('This revision has no files')
    expect(mock.manifest).not.toHaveBeenCalled()
    w.unmount()
  })
  it('aborts old loads and never applies a late resolution to a new revision', async () => {
    let resolveOld!: (value: string) => void
    mock.resolve
      .mockImplementationOnce(() => new Promise<string>((resolve) => (resolveOld = resolve)))
      .mockResolvedValue('new-revision')
    mock.manifest.mockResolvedValue({ entries: [entry('current')], nextPageToken: '', total: 1 })
    const w = show()
    await w.setProps({ rev: 'new' })
    await flushPromises()
    resolveOld('old-revision')
    await flushPromises()
    expect(mock.manifest).toHaveBeenCalledTimes(1)
    expect(mock.manifest).toHaveBeenCalledWith(
      'acme/demo',
      expect.objectContaining({ rev: 'new-revision' }),
      expect.anything(),
    )
    expect(w.text()).toContain('current')
    w.unmount()
  })
  it('stops a broken pagination token instead of looping', async () => {
    mock.manifest.mockResolvedValue({ entries: [entry('file')], nextPageToken: 'same', total: 100 })
    const w = show()
    await flushPromises()
    expect(mock.manifest).toHaveBeenCalledTimes(2)
    expect(w.text()).toContain('did not advance')
    w.unmount()
  })
})

describe('keyboard file selection', () => {
  async function results(paths = ['README.md', 'src/app.ts', 'src/util.ts']) {
    mock.manifest.mockResolvedValue({
      entries: paths.map(entry),
      nextPageToken: '',
      total: paths.length,
    })
    const wrapper = show()
    await flushPromises()
    const input = wrapper.get('input')
    input.element.focus()
    return { wrapper, input }
  }

  it('moves from search through non-first results, and back to search', async () => {
    const { wrapper, input } = await results()
    const links = wrapper.findAll('a')
    await input.trigger('keydown', { key: 'ArrowDown' })
    expect(document.activeElement).toBe(links[0]!.element)
    await links[0]!.trigger('keydown', { key: 'ArrowDown' })
    expect(document.activeElement).toBe(links[1]!.element)
    await links[1]!.trigger('keydown', { key: 'End' })
    expect(document.activeElement).toBe(links[2]!.element)
    await links[2]!.trigger('keydown', { key: 'ArrowUp' })
    expect(document.activeElement).toBe(links[1]!.element)
    await links[1]!.trigger('keydown', { key: 'End' })
    await links[2]!.trigger('keydown', { key: 'ArrowDown' })
    expect(document.activeElement).toBe(links[2]!.element)
    await links[2]!.trigger('keydown', { key: 'Home', ctrlKey: true })
    expect(document.activeElement).toBe(links[2]!.element)
    await links[2]!.trigger('keydown', { key: 'Home' })
    expect(document.activeElement).toBe(links[0]!.element)
    await links[0]!.trigger('keydown', { key: 'ArrowUp' })
    expect(document.activeElement).toBe(input.element)
    await input.trigger('keydown', { key: 'ArrowUp' })
    expect(document.activeElement).toBe(links[2]!.element)
    await links[2]!.trigger('click')
    expect(wrapper.emitted('select')).toEqual([['src/util.ts']])
    wrapper.unmount()
  })

  it.each([
    ['composition', { isComposing: true }],
    ['IME key code', { keyCode: 229 }],
    ['Alt', { altKey: true }],
    ['Ctrl', { ctrlKey: true }],
    ['Meta', { metaKey: true }],
    ['Shift', { shiftKey: true }],
  ])('does not hijack %s keyboard input', async (_name, modifier) => {
    const { wrapper, input } = await results()
    for (const key of ['Enter', 'ArrowDown', 'ArrowUp'])
      await input.trigger('keydown', { key, ...modifier })
    expect(wrapper.emitted('select')).toBeUndefined()
    expect(document.activeElement).toBe(input.element)
    wrapper.unmount()
  })

  it('ignores cursor movement and an empty result, then selects the current match', async () => {
    const { wrapper, input } = await results()
    await input.trigger('keydown', { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(input.element)
    await input.setValue('not-present')
    await input.trigger('keydown', { key: 'ArrowDown' })
    await input.trigger('keydown', { key: 'Enter' })
    expect(document.activeElement).toBe(input.element)
    expect(wrapper.emitted('select')).toBeUndefined()
    await input.setValue('src/util')
    await input.trigger('keydown', { key: 'Enter' })
    expect(wrapper.emitted('select')).toEqual([['src/util.ts']])
    wrapper.unmount()
  })

  it('keeps browser new-tab actions as native links', async () => {
    const { wrapper } = await results()
    const link = wrapper.findAll('a')[1]!
    await link.trigger('click', { ctrlKey: true })
    await link.trigger('click', { metaKey: true })
    expect(wrapper.emitted('select')).toBeUndefined()
    expect(link.attributes('href')).toContain('path=src%2Fapp.ts')
    wrapper.unmount()
  })
})
