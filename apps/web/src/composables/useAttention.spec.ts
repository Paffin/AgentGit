import { flushPromises, mount } from '@vue/test-utils'
import { defineComponent, h, ref, type Ref } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useAttention } from './useAttention'

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  proposals: vi.fn(),
  runs: vi.fn(),
  prepare: vi.fn(),
  detail: vi.fn(),
  auth: { isAuthenticated: true, identity: 'user:dana', can: () => true },
}))
vi.mock('@/api/instance', () => ({
  api: {
    repos: { list: mocks.list },
    proposals: { list: mocks.proposals, get: mocks.detail },
    agents: { runs: mocks.runs },
    integration: { prepare: mocks.prepare },
  },
}))
vi.mock('@/stores/auth', () => ({ useAuth: () => mocks.auth }))

let wrapper: ReturnType<typeof mount> | undefined
afterEach(() => {
  wrapper?.unmount()
  vi.clearAllMocks()
  vi.useRealTimers()
  mocks.auth.isAuthenticated = true
})
function setup(repository?: Ref<string>) {
  let attention!: ReturnType<typeof useAttention>
  wrapper = mount(
    defineComponent({
      setup() {
        attention = useAttention(repository)
        return () => h('div')
      },
    }),
  )
  return attention
}
function seed() {
  mocks.list.mockReset()
  mocks.list.mockResolvedValue({ items: [{ name: 'acme/repo', created_at_ms: 1 }] })
  mocks.proposals.mockResolvedValue({
    items: [
      {
        id: 'good',
        state: 'open',
        title: 'A ready change',
        author: 'user:alice',
        head: 'head',
        created_at_ms: 1,
      },
      {
        id: 'bad',
        state: 'open',
        title: 'Unavailable',
        author: 'user:alice',
        head: 'head',
        created_at_ms: 1,
      },
    ],
  })
  mocks.runs.mockResolvedValue([])
  mocks.detail.mockResolvedValue({ reviews: [] })
  mocks.prepare.mockImplementation(async (_repo: string, id: string) => {
    if (id === 'bad') throw new Error('temporary failure')
    return { blockers: [], conflicts: [] }
  })
}
describe('home attention data', () => {
  it('loads a selected older repository even when it is outside the first summary batch and catalogue page', async () => {
    seed()
    const recent = Array.from({ length: 30 }, (_, i) => ({
      name: `acme/new-${i}`,
      created_at_ms: i + 2,
    }))
    mocks.list
      .mockResolvedValueOnce({ items: recent, nextPageToken: 'older' })
      .mockResolvedValueOnce({
        items: [{ name: 'acme/repo', created_at_ms: 1 }],
        nextPageToken: '',
      })
    const state = setup(ref('acme/repo'))
    await flushPromises()
    expect(mocks.list).toHaveBeenCalledWith({ page_size: 200, page_token: 'older' })
    expect(mocks.proposals).toHaveBeenCalledTimes(1)
    expect(mocks.proposals).toHaveBeenCalledWith('acme/repo', expect.anything())
    expect(state.items.value.map((item) => item.title)).toEqual(['A ready change'])
    expect(state.skipped.value).toBe(0)
  })
  it('lets the user include more repositories without removing already loaded work', async () => {
    seed()
    mocks.list.mockResolvedValue({
      items: Array.from({ length: 40 }, (_, i) => ({ name: `acme/repo-${i}`, created_at_ms: i })),
      nextPageToken: '',
    })
    mocks.proposals.mockResolvedValue({ items: [], nextPageToken: '' })
    const state = setup()
    await flushPromises()
    expect(state.skipped.value).toBe(10)
    expect(mocks.proposals).toHaveBeenNthCalledWith(1, 'acme/repo-39', expect.anything())
    expect(mocks.proposals.mock.calls.some(([name]) => name === 'acme/repo-0')).toBe(false)
    await state.loadMore()
    expect(state.skipped.value).toBe(0)
    expect(new Set(mocks.proposals.mock.calls.map(([name]) => name)).size).toBe(40)
  })
  it('keeps readable work when another proposal cannot be loaded and marks the summary incomplete', async () => {
    seed()
    const state = setup()
    await flushPromises()
    expect(state.items.value.map((i) => i.title)).toEqual(['A ready change'])
    expect(state.unreadable.value).toBe(1)
    expect(state.settled.value).toBe(true)
  })
  it('makes an explicit refresh observable and retains the last successful refresh after failure', async () => {
    seed()
    const state = setup()
    await flushPromises()
    expect(state.lastUpdated.value).toBeGreaterThan(0)
    const last = state.lastUpdated.value
    mocks.list.mockRejectedValueOnce(new Error('offline'))
    await state.reload()
    expect(state.lastUpdated.value).toBe(last)
    expect(state.error.value).toBeInstanceOf(Error)
    expect(state.items.value).toHaveLength(1)
  })
  it('keeps proposals when the agent run list fails', async () => {
    seed()
    mocks.runs.mockRejectedValueOnce(new Error('runs unavailable'))
    const state = setup()
    await flushPromises()
    expect(state.items.value.map((i) => i.title)).toEqual(['A ready change'])
    expect(state.unreadable.value).toBe(1)
  })
  it('marks a truncated proposal page as incomplete', async () => {
    seed()
    mocks.prepare.mockResolvedValue({ blockers: [], conflicts: [] })
    mocks.proposals.mockResolvedValueOnce({ items: [], nextPageToken: 'more' })
    const state = setup()
    await flushPromises()
    expect(state.items.value).toEqual([])
    expect(state.unreadable.value).toBe(1)
  })
  it('does not supersede an unfinished refresh with background polling', async () => {
    seed()
    vi.useFakeTimers()
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    mocks.list.mockImplementationOnce(() => new Promise(() => undefined))
    setup()
    await vi.advanceTimersByTimeAsync(30_000)
    expect(mocks.list).toHaveBeenCalledTimes(1)
  })
  it('sends no requests after the session has ended', async () => {
    seed()
    mocks.auth.isAuthenticated = false
    const state = setup()
    await flushPromises()
    expect(mocks.list).not.toHaveBeenCalled()
    expect(state.loading.value).toBe(false)
  })
  it('never commits an older request after a newer refresh has completed', async () => {
    seed()
    let resolveOld!: (value: { items: { name: string; created_at_ms: number }[] }) => void
    mocks.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve
        }),
    )
    const state = setup()
    await state.reload()
    resolveOld({ items: [{ name: 'stale/repo', created_at_ms: 0 }] })
    await flushPromises()
    expect(state.repos.value.map((repo) => repo.name)).toEqual(['acme/repo'])
    expect(state.items.value.map((item) => item.title)).toEqual(['A ready change'])
  })
})
