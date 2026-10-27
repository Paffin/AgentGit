// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest'
import { startHeartbeats } from '../e2e/support/world'

afterEach(() => vi.unstubAllGlobals())

it('stops an idle heartbeat keeper without waiting for its next interval', async () => {
  vi.useFakeTimers()
  const keeper = startHeartbeats('http://127.0.0.1:1', 60_000)
  await Promise.resolve()
  await Promise.resolve()
  expect(vi.getTimerCount()).toBeGreaterThan(0)
  await keeper.stop()
  expect(vi.getTimerCount()).toBe(0)
}, 1_000)

it('can stop before the first heartbeat interval has been scheduled', async () => {
  vi.useFakeTimers()
  const keeper = startHeartbeats('http://127.0.0.1:1', 60_000)
  await keeper.stop()
  expect(vi.getTimerCount()).toBe(0)
}, 1_000)

it('sends heartbeats while running and schedules none after stopping', async () => {
  vi.useFakeTimers()
  const fetch = vi.fn<typeof globalThis.fetch>(
    async () => new Response('{}', { headers: { 'content-type': 'application/json' } }),
  )
  vi.stubGlobal('fetch', fetch)
  const keeper = startHeartbeats('http://127.0.0.1:1', 1_000)
  keeper.add({ token: 'test-only', repo: 'team/project', runId: 'run-test' })
  await vi.advanceTimersByTimeAsync(1_000)
  expect(fetch).toHaveBeenCalledTimes(1)
  expect(fetch.mock.calls[0]?.[0]).toContain('/v1/repos/team/project/runs/run-test/heartbeat')
  await keeper.stop()
  await vi.advanceTimersByTimeAsync(60_000)
  expect(fetch).toHaveBeenCalledTimes(1)
  expect(vi.getTimerCount()).toBe(0)
}, 1_000)
