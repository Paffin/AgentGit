import { expect, test } from './fixtures.ts'
import { expectAccessible } from './support/ui.ts'

// The root cause of the once-flaky axe failure `scrollable-region-focusable` (a `.wrap` of a table, dark theme):
// a table's content can become wider than its box WITHOUT the box resizing and WITHOUT the owning component
// re-rendering (a cell that fills in later, a child component that updates itself, a web font that finishes
// loading). The box then overflowed for a while without being the focusable, labelled region it must be, and an
// audit that ran in that window saw a scroll container the keyboard cannot reach. This test makes the window
// deterministic: it grows a cell from outside Vue, exactly like a child component would.

test.use({ colorScheme: 'dark' })

test('a table whose content grows later becomes a focusable region at once, and stops being one when it shrinks', async ({
  page,
}, testInfo) => {
  await page.goto('/access')
  await expect(page.getByTestId('mint-open')).toBeVisible()
  const wrap = page
    .locator('.wrap')
    .filter({ has: page.locator('tbody tr') })
    .first()
  await expect(wrap).toBeVisible()
  await expect(wrap).not.toHaveAttribute('tabindex', /.*/)

  // Grow the first cell by changing the DOM below the box, not through the owner's reactive state, and look
  // after a few microtask checkpoints in the same task: no frame, so no ResizeObserver callback, has run yet.
  // The audit must not be able to see the overflow without the region.
  const state = await wrap.evaluate(async (box) => {
    const cell = box.querySelector('tbody td') as HTMLElement
    const wide = document.createElement('span')
    wide.id = 'grown-by-hand'
    wide.style.display = 'inline-block'
    wide.style.width = '4000px'
    wide.textContent = 'a very wide cell'
    cell.appendChild(wide)
    for (let i = 0; i < 5; i++) await Promise.resolve() // MutationObserver, then Vue's patch
    return {
      overflows: box.scrollWidth > box.clientWidth + 13,
      tabindex: box.getAttribute('tabindex'),
      role: box.getAttribute('role'),
      name: box.getAttribute('aria-label'),
    }
  })
  expect(state.overflows).toBe(true)
  expect(state.tabindex).toBe('0')
  expect(state.role).toBe('region')
  expect(state.name).toMatch(/scrolls sideways/)
  await expectAccessible(page, testInfo, 'grown table')

  await page.locator('#grown-by-hand').evaluate((n) => n.remove())
  await expect(wrap).not.toHaveAttribute('tabindex', /.*/)
  await expectAccessible(page, testInfo, 'shrunk table')
})
