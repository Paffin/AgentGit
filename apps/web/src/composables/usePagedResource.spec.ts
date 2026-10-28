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
) {
  let list!: ReturnType<typeof usePagedResource<string>>
  const component = mount(
    defineComponent({
      setup() {
        list = usePagedResource(load, scope)
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
