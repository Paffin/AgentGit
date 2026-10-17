import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import { ApiError } from '@/api/errors'
import { useToasts } from './toasts'
import { usePrefs } from './prefs'

beforeEach(() => setActivePinia(createPinia()))

describe('toasts', () => {
  it('shows, auto-dismisses and caps the stack', () => {
    vi.useFakeTimers()
    const t = useToasts()
    t.ok('Saved')
    expect(t.items).toHaveLength(1)
    vi.advanceTimersByTime(5000)
    expect(t.items).toHaveLength(0)
    for (let i = 0; i < 8; i++) t.info(`n${i}`)
    expect(t.items.length).toBeLessThanOrEqual(5)
  })
  it('keeps errors around longer and carries the request id', () => {
    vi.useFakeTimers()
    const t = useToasts()
    t.error(
      'Could not save',
      new ApiError({ status: 500, code: 'internal', message: 'boom', requestId: 'req_9' }),
    )
    expect(t.items[0]).toMatchObject({ tone: 'bad', message: 'boom', requestId: 'req_9' })
    vi.advanceTimersByTime(5000)
    expect(t.items).toHaveLength(1)
    vi.advanceTimersByTime(5000)
    expect(t.items).toHaveLength(0)
  })
  it('pauses while hovered and resumes afterwards', () => {
    vi.useFakeTimers()
    const t = useToasts()
    const id = t.ok('Hold on')
    t.pause(id)
    vi.advanceTimersByTime(60_000)
    expect(t.items).toHaveLength(1)
    t.resume(id)
    vi.advanceTimersByTime(5000)
    expect(t.items).toHaveLength(0)
  })
  it('dismisses by id and clears', () => {
    const t = useToasts()
    const a = t.warn('a')
    t.info('b')
    t.dismiss(a)
    expect(t.items.map((x) => x.title)).toEqual(['b'])
    t.clear()
    expect(t.items).toEqual([])
  })
})

describe('preferences', () => {
  it('applies the theme to the document and remembers it', async () => {
    const p = usePrefs()
    expect(p.theme).toBe('system')
    p.theme = 'dark'
    await nextTick()
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark')
    expect(window.localStorage.getItem('agentgit.theme')).toBe('dark')
    p.theme = 'system'
    await nextTick()
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false)
  })
  it('cycles system → light → dark → system', () => {
    const p = usePrefs()
    p.cycleTheme()
    expect(p.theme).toBe('light')
    p.cycleTheme()
    expect(p.theme).toBe('dark')
    p.cycleTheme()
    expect(p.theme).toBe('system')
  })
  it('reads saved preferences, ignores garbage', () => {
    window.localStorage.setItem('agentgit.theme', 'light')
    window.localStorage.setItem('agentgit.staleAfterSec', '45')
    const p = usePrefs()
    expect(p.theme).toBe('light')
    expect(p.staleAfterMs).toBe(45_000)
    setActivePinia(createPinia())
    window.localStorage.setItem('agentgit.theme', 'neon')
    window.localStorage.setItem('agentgit.staleAfterSec', 'abc')
    const q = usePrefs()
    expect(q.theme).toBe('system')
    expect(q.staleAfterMs).toBe(120_000)
  })
})
