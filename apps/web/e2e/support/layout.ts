// Layout measurements taken inside the page, for the responsive checks. They look at what the browser really
// laid out (bounding boxes, computed styles), not at the source.
import type { Locator, Page } from '@playwright/test'

export interface Overflow {
  /** `document.documentElement.scrollWidth`: more than `width` means the page scrolls sideways. */
  scrollWidth: number
  width: number
  /** Elements whose visible part (after clipping by their scroll containers) sticks out of the viewport. */
  offenders: string[]
}

/** Does anything make the page wider than the window? */
export async function measureOverflow(page: Page): Promise<Overflow> {
  return page.evaluate(() => {
    const width = document.documentElement.clientWidth
    const describe = (el: Element) => {
      const id = el.id ? `#${el.id}` : ''
      const cls =
        typeof el.className === 'string' && el.className ? `.${el.className.split(/\s+/)[0]}` : ''
      const text = (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40)
      return `${el.tagName.toLowerCase()}${id}${cls} "${text}"`
    }
    const offenders: string[] = []
    for (const el of Array.from(document.querySelectorAll('body *'))) {
      const style = getComputedStyle(el)
      if (style.display === 'none' || style.visibility === 'hidden') continue
      if (style.position === 'fixed') continue
      let rect = el.getBoundingClientRect()
      if (rect.width === 0 || rect.height === 0) continue
      // Clip by every ancestor that clips (scroll containers, overflow: hidden): what they hide is not overflow.
      for (let p = el.parentElement; p && p !== document.documentElement; p = p.parentElement) {
        const ps = getComputedStyle(p)
        if (ps.overflowX !== 'visible') {
          const pr = p.getBoundingClientRect()
          rect = new DOMRect(
            Math.max(rect.left, pr.left),
            rect.top,
            Math.max(0, Math.min(rect.right, pr.right) - Math.max(rect.left, pr.left)),
            rect.height,
          )
        }
      }
      if (rect.width > 0 && (rect.right > width + 1 || rect.left < -1)) {
        offenders.push(describe(el))
      }
    }
    return {
      scrollWidth: document.documentElement.scrollWidth,
      width,
      offenders: offenders.slice(0, 8),
    }
  })
}

export interface Target {
  what: string
  width: number
  height: number
}

/**
 * The size of every visible control matching `selector`. Controls inside `.sr-only` and zero-size ones are skipped.
 * `labelAsTarget` measures a checkbox or radio by its label (the label is what a finger hits).
 */
export async function measureTargets(page: Page, selector: string): Promise<Target[]> {
  return page.evaluate((sel) => {
    const out: { what: string; width: number; height: number }[] = []
    for (const el of Array.from(document.querySelectorAll(sel))) {
      const style = getComputedStyle(el)
      if (style.display === 'none' || style.visibility === 'hidden') continue
      let box = el as HTMLElement
      if (el instanceof HTMLInputElement && (el.type === 'checkbox' || el.type === 'radio')) {
        box = el.closest('label') ?? el.labels?.[0] ?? box
      }
      const r = box.getBoundingClientRect()
      if (r.width === 0 || r.height === 0) continue
      const name =
        el.getAttribute('aria-label') ??
        (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 30)
      out.push({
        what: `${el.tagName.toLowerCase()}[${el.getAttribute('role') ?? el.getAttribute('type') ?? ''}] "${name}"`,
        width: Math.round(r.width),
        height: Math.round(r.height),
      })
    }
    return out
  }, selector)
}

/** Targets smaller than `min` CSS pixels in either direction. */
export async function smallTargets(page: Page, selector: string, min = 40): Promise<string[]> {
  return (await measureTargets(page, selector))
    .filter((t) => t.height < min - 0.5 || t.width < min - 0.5)
    .map((t) => `${t.what} is ${t.width}x${t.height}`)
}

/** Is the whole of `locator` inside the viewport (after scrolling it into view if it can be scrolled to)? */
export async function fitsViewport(page: Page, locator: Locator): Promise<boolean> {
  await locator.scrollIntoViewIfNeeded()
  const box = await locator.boundingBox()
  const size = page.viewportSize()
  if (!box || !size) return false
  return (
    box.x >= -1 &&
    box.y >= -1 &&
    box.x + box.width <= size.width + 1 &&
    box.y + box.height <= size.height + 1
  )
}
