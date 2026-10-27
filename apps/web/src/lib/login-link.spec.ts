import { describe, expect, it, vi } from 'vitest'
import { takeLoginFragment } from './login-link'

function where(hash: string, pathname = '/repos/demo/welcome', search = '') {
  const replaceState = vi.fn()
  return {
    location: { hash, pathname, search },
    history: { replaceState, state: { k: 1 } },
    replaceState,
  }
}

describe('takeLoginFragment', () => {
  it('returns the credential and removes it from the address bar, keeping path and query', () => {
    const w = where('#token=agt_secret_0123456789', '/repos/demo/welcome', '?tab=files')
    expect(takeLoginFragment(w.location, w.history)).toBe('agt_secret_0123456789')
    expect(w.replaceState).toHaveBeenCalledTimes(1)
    const [state, , url] = w.replaceState.mock.calls[0] ?? []
    expect(state).toEqual({ k: 1 })
    expect(url).toBe('/repos/demo/welcome?tab=files')
    expect(String(url)).not.toContain('agt_')
  })

  it('leaves other fragment parameters alone', () => {
    const w = where('#a=1&token=agt_x_0123456789&b=2')
    expect(takeLoginFragment(w.location, w.history)).toBe('agt_x_0123456789')
    expect(w.replaceState.mock.calls[0]?.[2]).toBe('/repos/demo/welcome#a=1&b=2')
  })

  it('does nothing when there is no credential in the fragment', () => {
    for (const hash of ['', '#', '#section', '#a=1']) {
      const w = where(hash)
      expect(takeLoginFragment(w.location, w.history)).toBeNull()
      expect(w.replaceState).not.toHaveBeenCalled()
    }
  })

  it('removes an empty credential parameter too, and reports no credential', () => {
    const w = where('#token=')
    expect(takeLoginFragment(w.location, w.history)).toBeNull()
    expect(w.replaceState.mock.calls[0]?.[2]).toBe('/repos/demo/welcome')
  })

  it('decodes a percent-encoded credential', () => {
    const w = where('#token=agt_a%2Bb_0123456789')
    expect(takeLoginFragment(w.location, w.history)).toBe('agt_a+b_0123456789')
  })
})
