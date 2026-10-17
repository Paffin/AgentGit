import { afterEach, vi } from 'vitest'
import { resetServerClock } from '../src/api/clock'

// happy-dom has no matchMedia / ResizeObserver; components that adapt to them degrade gracefully, but give them stubs.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia
}

afterEach(() => {
  vi.useRealTimers()
  resetServerClock()
  document.body.replaceChildren()
  document.documentElement.removeAttribute('data-theme')
  try {
    window.sessionStorage.clear()
    window.localStorage.clear()
  } catch {
    /* storage may be unavailable */
  }
})
