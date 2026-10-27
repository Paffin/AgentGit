// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { observeServerDate, resetServerClock, serverNow, serverOffset } from './clock'

describe('server clock', () => {
  it('treats the 1 s resolution of the Date header as noise', () => {
    resetServerClock()
    const now = 1_000_000_000_000
    observeServerDate(now - 900, now) // header truncated to the second, true time within a second of ours
    expect(serverOffset()).toBe(0)
    expect(serverNow(now)).toBe(now)
  })

  it('corrects a real skew', () => {
    resetServerClock()
    const now = 1_000_000_000_000
    observeServerDate(now + 60_000, now) // the server is a minute ahead
    expect(serverOffset()).toBeGreaterThan(59_000)
    expect(serverNow(now)).toBeGreaterThan(now + 59_000)
  })

  it('forgets a skew that went away', () => {
    const now = 1_000_000_000_000
    observeServerDate(now - 120_000, now)
    expect(serverOffset()).toBeLessThan(0)
    observeServerDate(now, now)
    expect(serverOffset()).toBe(0)
  })

  it('ignores garbage', () => {
    resetServerClock()
    observeServerDate(Number.NaN)
    expect(serverOffset()).toBe(0)
  })
})
