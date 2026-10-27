import { afterEach, describe, expect, it } from 'vitest'
import { modalLayer } from './modalLayer'
afterEach(() => {
  document.documentElement.style.overflow = ''
  document.body.replaceChildren()
})
function level(value: string) {
  const match = value.match(/ ([+-]) (\d+)\)/)!
  return Number(match[2]) * (match[1] === '-' ? -1 : 1)
}
describe('modal ownership', () => {
  it('keeps the latest layer above the earlier one and restores ownership when it closes', () => {
    const first = modalLayer(),
      second = modalLayer()
    try {
      first.activate()
      first.activate()
      second.activate()
      expect(first.top.value).toBe(false)
      expect(second.top.value).toBe(true)
      expect(level(second.zIndex.value)).toBeGreaterThan(level(first.zIndex.value))
      second.deactivate()
      expect(first.top.value).toBe(true)
    } finally {
      second.deactivate()
      first.deactivate()
    }
  })
  it('works without an app element and restores the previous scroll policy', () => {
    document.documentElement.style.overflow = 'scroll'
    const layer = modalLayer()
    try {
      expect(() => layer.activate()).not.toThrow()
      expect(document.documentElement.style.overflow).toBe('hidden')
    } finally {
      layer.deactivate()
    }
    expect(document.documentElement.style.overflow).toBe('scroll')
  })
  it('keeps an already inert app inert after the last dialog closes', () => {
    const root = document.createElement('div')
    root.id = 'app'
    root.setAttribute('inert', '')
    document.body.append(root)
    const layer = modalLayer()
    try {
      layer.activate()
    } finally {
      layer.deactivate()
    }
    expect(root.hasAttribute('inert')).toBe(true)
  })
})
