import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, ref } from 'vue'
import { enableAutoUnmount, mount } from '@vue/test-utils'
import { useResource } from './useResource'
import type { Resource } from './useResource'
import { ApiError } from '@/api/errors'

enableAutoUnmount(afterEach)

function host<T>(make: () => Resource<T>) {
  let res!: Resource<T>
  const w = mount(
    defineComponent({
      setup() {
        res = make()
        return () => h('div')
      },
    }),
  )
  return { w, res: () => res }
}

const settle = async () => {
  for (let i = 0; i < 4; i++) await nextTick()
}

describe('useResource', () => {
  it('does not start a deferred first request after its owner unmounts', async () => {
    const load = vi.fn().mockResolvedValue('unwanted')
    const { w, res } = host(() => useResource(load, { immediate: false }))
    w.unmount()
    await res().reload()
    expect(load).not.toHaveBeenCalled()
    expect(res().data.value).toBeUndefined()
  })

  it('aborts pending work and ignores later reloads and results after unmount', async () => {
    let finish!: (value: string) => void
    let signal!: AbortSignal
    const load = vi.fn((requestSignal: AbortSignal) => {
      signal = requestSignal
      return new Promise<string>((resolve) => {
        finish = resolve
      })
    })
    const { w, res } = host(() => useResource(load))
    w.unmount()
    expect(signal.aborted).toBe(true)
    const firstReload = res().reload()
    expect(load).toHaveBeenCalledTimes(1)
    await firstReload
    await res().reload()
    expect(load).toHaveBeenCalledTimes(1)
    finish('obsolete')
    await settle()
    expect(res().data.value).toBeUndefined()
    expect(res().error.value).toBeNull()
  })

  it('loads once and keeps the data', async () => {
    const load = vi.fn().mockResolvedValue(42)
    const { res } = host(() => useResource(load))
    await settle()
    expect(load).toHaveBeenCalledTimes(1)
    expect(res().data.value).toBe(42)
    expect(res().settled.value).toBe(true)
  })

  it('reloads when a watched source changes and drops the stale data meanwhile', async () => {
    const id = ref(1)
    let release: () => void = () => undefined
    const load = vi.fn(async () => {
      if (id.value === 2) await new Promise<void>((r) => (release = r))
      return id.value * 10
    })
    const { res } = host(() => useResource(load, { watch: [id] }))
    await settle()
    expect(res().data.value).toBe(10)
    id.value = 2
    await settle()
    expect(res().data.value).toBeUndefined()
    expect(res().loading.value).toBe(true)
    release()
    await settle()
    expect(res().data.value).toBe(20)
  })

  it('requests nothing while it is not enabled, then loads when its input arrives', async () => {
    const stream = ref('')
    const load = vi.fn(async () => `policy of ${stream.value}`)
    const { res } = host(() =>
      useResource(load, { watch: [stream], enabled: () => stream.value !== '' }),
    )
    await settle()
    expect(load).not.toHaveBeenCalled()
    expect(res().loading.value).toBe(false)
    stream.value = 'main'
    await settle()
    expect(load).toHaveBeenCalledTimes(1)
    expect(res().data.value).toBe('policy of main')
  })

  it('lets the newest load win over an older, slower one', async () => {
    const id = ref(1)
    const resolvers: Record<number, (v: string) => void> = {}
    const load = vi.fn(
      () =>
        new Promise<string>((r) => {
          resolvers[id.value] = r
        }),
    )
    const { res } = host(() => useResource(load, { watch: [id] }))
    await nextTick()
    id.value = 2
    await settle()
    resolvers[2]!('second')
    await settle()
    resolvers[1]!('first')
    await settle()
    expect(res().data.value).toBe('second')
  })

  it('ignores a failed older request after a newer request succeeds', async () => {
    let fail!: (error: Error) => void
    const load = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, reject) => {
            fail = reject
          }),
      )
      .mockResolvedValue('current')
    const { res } = host(() => useResource(load))
    await res().reload()
    fail(new Error('obsolete failure'))
    await settle()
    expect(res().error.value).toBeNull()
    expect(res().data.value).toBe('current')
  })

  it('does not display a cancelled request as an error', async () => {
    const { res } = host(() =>
      useResource(async () => {
        throw new ApiError({ status: 0, code: 'cancelled', message: 'Cancelled' })
      }),
    )
    await settle()
    expect(res().error.value).toBeNull()
  })

  it('keeps the error of a failed load', async () => {
    const { res } = host(() => useResource(async () => Promise.reject(new Error('boom'))))
    await settle()
    expect((res().error.value as Error).message).toBe('boom')
    expect(res().settled.value).toBe(true)
  })
})
