import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import type { WatchSource } from 'vue'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import type { Page } from '@/api/types'
import { usePagedResource } from './usePagedResource'

enableAutoUnmount(afterEach)

function host(
  load: (cursor: string, signal: AbortSignal) => Promise<Page<string>>,
  scope: WatchSource[],
  enabled: () => boolean = () => true,
) {
  let list!: ReturnType<typeof usePagedResource<string>>
  const component = mount(
    defineComponent({
      setup() {
        list = usePagedResource(load, scope, enabled)
        return () => h('div')
      },
    }),
  )
  return { list, component }
}

it('loads older history and preserves its depth across live refreshes', async () => {
  const load = vi.fn(async (cursor: string) => ({
    items: [cursor || 'newest'],
    nextPageToken: cursor ? '' : 'older',
  }))
  const { list } = host(load, [])
  await flushPromises()
  await list.more()
  expect(list.data.value).toEqual(['newest', 'older'])
  expect(list.next.value).toBe('')
  await list.reload()
  expect(list.data.value).toEqual(['newest', 'older'])
  expect(load.mock.calls.map(([cursor]) => cursor)).toEqual(['', 'older', '', 'older'])
})

it('drops account-scoped history synchronously and refuses late pages or work after unmount', async () => {
  const account = ref('first')
  let finish!: (page: Page<string>) => void
  let requestSignal!: AbortSignal
  const load = vi.fn((cursor: string, signal: AbortSignal): Promise<Page<string>> => {
    if (!cursor) return Promise.resolve({ items: [account.value], nextPageToken: 'older' })
    requestSignal = signal
    return new Promise((resolve) => {
      finish = resolve
    })
  })
  const { list, component } = host(load, [account])
  await flushPromises()
  const pending = list.more()
  account.value = 'second'
  expect(list.data.value).toEqual([])
  expect(requestSignal.aborted).toBe(true)
  await flushPromises()
  finish({ items: ['first-private-history'], nextPageToken: '' })
  await pending
  expect(list.data.value).toEqual(['second'])
  component.unmount()
  const calls = load.mock.calls.length
  await list.reload()
  expect(load).toHaveBeenCalledTimes(calls)
})

it('does not fetch while disabled and discards a page after disabling', async () => {
  const enabled = ref(false)
  let finish!: (page: Page<string>) => void
  let signal!: AbortSignal
  const load = vi.fn((_cursor: string, request: AbortSignal) => {
    signal = request
    return new Promise<Page<string>>((resolve) => {
      finish = resolve
    })
  })
  const { list } = host(load, [enabled], () => enabled.value)
  expect(load).not.toHaveBeenCalled()
  enabled.value = true
  expect(load).toHaveBeenCalledTimes(1)
  enabled.value = false
  expect(signal.aborted).toBe(true)
  finish({ items: ['obsolete'], nextPageToken: '' })
  await flushPromises()
  expect(list.data.value).toEqual([])
  expect(list.loading.value).toBe(false)
})
