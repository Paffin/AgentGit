import { describe, expect, it, vi } from 'vitest'
import {
  baseName,
  decodeUtf8,
  dirName,
  imageMime,
  looksBinary,
  splitLines,
  toDataUrl,
} from './text'
import { coalesce, mapLimit } from './pool'
import { readStore, removeStore, writeStore } from './storage'

describe('text helpers', () => {
  it('detects binary by NUL byte or invalid UTF-8', () => {
    expect(looksBinary(new TextEncoder().encode('hello\nworld'))).toBe(false)
    expect(looksBinary(new Uint8Array([104, 0, 105]))).toBe(true)
    expect(looksBinary(new Uint8Array([0xff, 0xfe, 0xfd]))).toBe(true)
    expect(looksBinary(new TextEncoder().encode('héllo ✓'))).toBe(false)
    expect(looksBinary(new Uint8Array())).toBe(false)
  })
  it('decodes and splits lines', () => {
    expect(decodeUtf8(new TextEncoder().encode('a\nb'))).toBe('a\nb')
    expect(splitLines('a\nb\n')).toEqual(['a', 'b'])
    expect(splitLines('a\n\nb')).toEqual(['a', '', 'b'])
    expect(splitLines('')).toEqual([])
  })
  it('previews raster images only', () => {
    expect(imageMime('a/b.PNG')).toBe('image/png')
    expect(imageMime('x.svg')).toBeNull()
    expect(imageMime('noext')).toBeNull()
    expect(toDataUrl(new Uint8Array([1, 2, 3]), 'image/png')).toBe('data:image/png;base64,AQID')
  })
  it('splits paths', () => {
    expect(baseName('a/b/c.ts')).toBe('c.ts')
    expect(dirName('a/b/c.ts')).toBe('a/b')
    expect(dirName('c.ts')).toBe('')
  })
})

describe('mapLimit', () => {
  it('never runs more than the limit at once and keeps order', async () => {
    let active = 0
    let peak = 0
    const res = await mapLimit([1, 2, 3, 4, 5, 6], 2, async (n) => {
      active++
      peak = Math.max(peak, active)
      await new Promise((r) => setTimeout(r, 5))
      active--
      return n * 2
    })
    expect(peak).toBe(2)
    expect(res.map((r) => (r.status === 'fulfilled' ? r.value : null))).toEqual([
      2, 4, 6, 8, 10, 12,
    ])
  })
  it('reports failures without rejecting', async () => {
    const res = await mapLimit([1, 2], 2, async (n) => {
      if (n === 2) throw new Error('bad')
      return n
    })
    expect(res[0]!.status).toBe('fulfilled')
    expect(res[1]!.status).toBe('rejected')
    expect(await mapLimit([], 3, async () => 1)).toEqual([])
  })
})

describe('coalesce', () => {
  it('collapses a burst into one run', async () => {
    vi.useFakeTimers()
    const fn = vi.fn(async () => undefined)
    const c = coalesce(fn, 100)
    c.trigger()
    c.trigger()
    c.trigger()
    await vi.advanceTimersByTimeAsync(99)
    expect(fn).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(2)
    expect(fn).toHaveBeenCalledTimes(1)
  })
  it('runs once more when triggered during a run', async () => {
    vi.useFakeTimers()
    let release: () => void = () => undefined
    const fn = vi.fn(() => new Promise<void>((r) => (release = r)))
    const c = coalesce(fn, 10)
    c.trigger()
    await vi.advanceTimersByTimeAsync(11)
    expect(fn).toHaveBeenCalledTimes(1)
    c.trigger() // while running
    release()
    await vi.advanceTimersByTimeAsync(30)
    expect(fn).toHaveBeenCalledTimes(2)
  })
  it('can be cancelled', async () => {
    vi.useFakeTimers()
    const fn = vi.fn()
    const c = coalesce(fn, 10)
    c.trigger()
    c.cancel()
    await vi.advanceTimersByTimeAsync(50)
    expect(fn).not.toHaveBeenCalled()
  })
})

describe('storage', () => {
  it('reads, writes and removes', () => {
    expect(readStore('local', 'k')).toBeNull()
    expect(writeStore('local', 'k', 'v')).toBe(true)
    expect(readStore('local', 'k')).toBe('v')
    removeStore('local', 'k')
    expect(readStore('local', 'k')).toBeNull()
  })
  it('never throws when storage is blocked', () => {
    const spy = vi.spyOn(window, 'sessionStorage', 'get').mockImplementation(() => {
      throw new DOMException('blocked', 'SecurityError')
    })
    expect(readStore('session', 'k')).toBeNull()
    expect(writeStore('session', 'k', 'v')).toBe(false)
    expect(() => removeStore('session', 'k')).not.toThrow()
    spy.mockRestore()
  })
})
