import { flushPromises, mount } from '@vue/test-utils'
import { defineComponent, h, reactive, ref } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useAttentionFeed } from './useAttentionFeed'
import type { FeedOptions } from '@/api/sse'

const mocks = vi.hoisted(() => ({ open: vi.fn(), auth: {} as Record<string, unknown> }))
vi.mock('@/api/instance', () => ({ client: {} }))
vi.mock('@/api/sse', () => ({ openEventFeed: mocks.open }))
vi.mock('@/stores/auth', () => ({ useAuth: () => mocks.auth }))
let wrapper: ReturnType<typeof mount> | undefined

afterEach(() => {
  wrapper?.unmount()
  vi.restoreAllMocks()
  vi.clearAllMocks()
})

describe('selected repository attention feed', () => {
  it('opens only one selected repository, pauses hidden tabs and ignores replaced callbacks', async () => {
    let visibility = 'visible'
    vi.spyOn(document, 'visibilityState', 'get').mockImplementation(
      () => visibility as DocumentVisibilityState,
    )
    mocks.auth = reactive({ isAuthenticated: true, token: 'first' })
    const feeds: { options: FeedOptions; close: ReturnType<typeof vi.fn> }[] = []
    mocks.open.mockImplementation((options: FeedOptions) => {
      const close = vi.fn()
      feeds.push({ options, close })
      return { close }
    })
    const repo = ref(''),
      refresh = vi.fn()
    wrapper = mount(
      defineComponent({
        setup() {
          useAttentionFeed(repo, refresh)
          return () => h('div')
        },
      }),
    )
    expect(feeds).toHaveLength(0)
    repo.value = 'acme/one'
    await flushPromises()
    expect(feeds).toHaveLength(1)
    expect(feeds[0]!.options.session).toEqual({ token: 'first', quietUnauthenticated: true })
    feeds[0]!.options.onStatus?.('live', { attempt: 0 })
    expect(refresh).toHaveBeenCalledTimes(1)
    repo.value = 'acme/two'
    await flushPromises()
    expect(feeds[0]!.close).toHaveBeenCalledOnce()
    feeds[0]!.options.onStatus?.('live', { attempt: 0 })
    feeds[0]!.options.onEvent({
      repo: 'acme/one',
      seq: 1,
      timestamp_ms: 1,
      topic: 'proposal.create',
    })
    expect(refresh).toHaveBeenCalledTimes(1)
    visibility = 'hidden'
    document.dispatchEvent(new Event('visibilitychange'))
    await flushPromises()
    expect(feeds[1]!.close).toHaveBeenCalledOnce()
    visibility = 'visible'
    document.dispatchEvent(new Event('visibilitychange'))
    await flushPromises()
    expect(feeds).toHaveLength(3)
    mocks.auth.token = 'second'
    await flushPromises()
    expect(feeds[2]!.close).toHaveBeenCalledOnce()
    expect(feeds).toHaveLength(4)
    expect(feeds[3]!.options.session?.token).toBe('second')
    wrapper.unmount()
    wrapper = undefined
    expect(feeds[3]!.close).toHaveBeenCalledOnce()
    feeds[3]!.options.onStatus?.('live', { attempt: 0 })
    expect(refresh).toHaveBeenCalledTimes(1)
  })
})
