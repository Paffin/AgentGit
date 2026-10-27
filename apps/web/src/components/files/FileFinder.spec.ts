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
