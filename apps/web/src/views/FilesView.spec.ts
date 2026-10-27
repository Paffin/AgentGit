import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import FilesView from './FilesView.vue'

const api = vi.hoisted(() => ({ resolve: vi.fn(), list: vi.fn() }))
vi.mock('@/api/instance', () => ({ api: { tree: api } }))
vi.mock('@/stores/auth', () => ({ useAuth: () => ({ can: () => false, identity: 'user:reader' }) }))
vi.mock('@/stores/board', () => ({
  useBoard: () => ({
    repo: { default_stream: 'main' },
    load: { repo: { status: 'ready' } },
    streams: [],
  }),
}))
const page = (name: string, nextPageToken = '') => ({
  entries: [{ name, kind: 'file', size: 1, executable: false, blob_id: '' }],
  nextPageToken,
})
async function show() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/repos/:owner/:name/files', component: FilesView }],
  })
  await router.push('/repos/acme/demo/files?rev=main&path=old')
  const w = mount(FilesView, {
    global: {
      plugins: [router],
      stubs: {
        FileFinder: true,
        FileViewer: true,
        FileUpload: true,
        FileEditor: true,
        MissingFileDraft: true,
      },
    },
  })
  await flushPromises()
  return { w, router }
}
beforeEach(() => {
  api.resolve.mockReset().mockResolvedValue({ kind: 'dir' })
  api.list.mockReset().mockImplementation(async (_repo, query) => page(`${query.path}-file.txt`))
})

describe('directory identity', () => {
  it('removes old links while the next folder is still being resolved', async () => {
    api.list.mockResolvedValueOnce(page('old-file.txt', 'old-page-token'))
    const { w, router } = await show()
    expect(w.get('a.entry').text()).toContain('old-file.txt')
    let finish!: (value: { kind: string }) => void
    api.resolve.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await router.push({ query: { rev: 'main', path: 'new' } })
    await flushPromises()
    expect(w.findAll('a.entry')).toHaveLength(0)
    expect(w.text()).not.toContain('old-file.txt')
    expect(w.findAll('button').some((button) => button.text() === 'Load more')).toBe(false)
    finish({ kind: 'dir' })
    await flushPromises()
    expect(w.get('a.entry').attributes('href')).toContain('path=new/new-file.txt')
    w.unmount()
  })

  it.each([
    ['repository', '/repos/other/repository/files?rev=main&path=old'],
    ['revision', '/repos/acme/demo/files?rev=release&path=old'],
  ])('does not show old entries while a new %s list loads', async (_name, url) => {
    const { w, router } = await show()
    let finish!: (value: ReturnType<typeof page>) => void
    api.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await router.push(url)
    await flushPromises()
    expect(w.findAll('a.entry')).toHaveLength(0)
    finish(page('current.txt'))
    await flushPromises()
    expect(w.get('a.entry').text()).toContain('current.txt')
    w.unmount()
  })

  it('ignores a late directory response after moving to a file', async () => {
    const { w, router } = await show()
    let finish!: (value: ReturnType<typeof page>) => void
    api.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await router.push({ query: { rev: 'main', path: 'slow-folder' } })
    await flushPromises()
    api.resolve.mockResolvedValueOnce({ kind: 'file' })
    await router.push({ query: { rev: 'main', path: 'file.txt' } })
    await flushPromises()
    let finishNext!: (value: { kind: string }) => void
    api.resolve.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishNext = resolve
        }),
    )
    await router.push({ query: { rev: 'main', path: 'last' } })
    await flushPromises()
    finish(page('stale.txt'))
    await flushPromises()
    expect(w.text()).not.toContain('stale.txt')
    finishNext({ kind: 'dir' })
    await flushPromises()
    expect(w.get('a.entry').text()).toContain('last-file.txt')
    w.unmount()
  })

  it('keeps the current directory entries when loading its next page', async () => {
    api.list
      .mockResolvedValueOnce(page('first.txt', 'next-page'))
      .mockResolvedValueOnce(page('second.txt'))
    const { w } = await show()
    await w
      .findAll('button')
      .find((button) => button.text() === 'Load more')!
      .trigger('click')
    await flushPromises()
    expect(w.findAll('a.entry').map((link) => link.text())).toEqual(['first.txt', 'second.txt'])
    expect(api.list).toHaveBeenLastCalledWith(
      'acme/demo',
      expect.objectContaining({ path: 'old', page_token: 'next-page' }),
    )
    w.unmount()
  })

  it('retries the current folder after an error without reviving the previous folder', async () => {
    const { w, router } = await show()
    api.list.mockRejectedValueOnce(new Error('Folder unavailable'))
    await router.push({ query: { rev: 'main', path: 'broken' } })
    await flushPromises()
    expect(w.text()).toContain('Folder unavailable')
    const retry = w.findAll('button').find((button) => button.text() === 'Try again')!
    let finish!: (value: ReturnType<typeof page>) => void
    api.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await retry.trigger('click')
    expect(w.findAll('a.entry')).toHaveLength(0)
    finish(page('recovered.txt'))
    await flushPromises()
    expect(w.get('a.entry').text()).toContain('recovered.txt')
    w.unmount()
  })
})

describe('directory pagination recovery', () => {
  it('keeps loaded pages and retries the same cursor without duplicates', async () => {
    api.list
      .mockResolvedValueOnce(page('first.txt', 'page-two'))
      .mockResolvedValueOnce(page('second.txt', 'page-three'))
      .mockRejectedValueOnce(new Error('Temporary listing failure'))
      .mockResolvedValueOnce(page('third.txt'))
    const { w } = await show()
    const more = () => w.findAll('button').find((button) => button.text() === 'Load more')!
    await more().trigger('click')
    await flushPromises()
    await more().trigger('click')
    await flushPromises()
    expect(w.findAll('a.entry').map((link) => link.text())).toEqual(['first.txt', 'second.txt'])
    expect(w.text()).toContain('Temporary listing failure')
    await more().trigger('click')
    await flushPromises()
    expect(api.list.mock.calls[2]![1].page_token).toBe('page-three')
    expect(api.list.mock.calls[3]![1].page_token).toBe('page-three')
    expect(w.findAll('a.entry').map((link) => link.text())).toEqual([
      'first.txt',
      'second.txt',
      'third.txt',
    ])
    expect(w.text()).not.toContain('Temporary listing failure')
    w.unmount()
  })

  it('discards a pending retry after navigating to a different directory', async () => {
    api.list
      .mockResolvedValueOnce(page('first.txt', 'next'))
      .mockRejectedValueOnce(new Error('Try later'))
    const { w, router } = await show()
    const more = () => w.findAll('button').find((button) => button.text() === 'Load more')!
    await more().trigger('click')
    await flushPromises()
    let finish!: (value: ReturnType<typeof page>) => void
    api.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await more().trigger('click')
    await router.push({ query: { rev: 'main', path: 'new' } })
    await flushPromises()
    finish(page('old-retry.txt'))
    await flushPromises()
    expect(w.findAll('a.entry').map((link) => link.text())).toEqual(['new-file.txt'])
    expect(w.text()).not.toContain('Try later')
    w.unmount()
  })
})
