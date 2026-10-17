// Small helpers for driving and checking the UI.
import AxeBuilder from '@axe-core/playwright'
import { expect } from '@playwright/test'
import type { Locator, Page, TestInfo } from '@playwright/test'

/** Waits for the live indicator to show a working SSE connection. */
export async function waitLive(page: Page): Promise<void> {
  await expect(page.getByTestId('live-indicator')).toContainText(/live/i, { timeout: 15_000 })
}

/** The page's main landmark, the place every screen's content lives. */
export const main = (page: Page): Locator => page.locator('main')

/** The four verdicts of a proposal, as `{ mergeable: 'ok', validated: 'failed', ... }`. */
export async function verdicts(scope: Locator): Promise<Record<string, string>> {
  const out: Record<string, string> = {}
  const pills = scope.locator('[data-state]')
  const n = await pills.count()
  for (let i = 0; i < n; i++) {
    const pill = pills.nth(i)
    out[(await pill.getAttribute('data-state')) ?? '?'] =
      (await pill.getAttribute('data-tone')) ?? '?'
  }
  return out
}

type AxePage = ConstructorParameters<typeof AxeBuilder>[0]['page']

interface AxeNode {
  target: unknown
  html: string
  failureSummary?: string
}
interface AxeViolation {
  id: string
  impact?: string | null
  help: string
  helpUrl: string
  nodes: AxeNode[]
}

/** Fails on any serious or critical accessibility violation (WCAG 2.x A and AA plus best practices). */
export async function expectAccessible(
  page: Page,
  testInfo: TestInfo,
  label: string,
  opts: { exclude?: string[] } = {},
): Promise<void> {
  let builder = new AxeBuilder({ page: page as unknown as AxePage }).withTags([
    'wcag2a',
    'wcag2aa',
    'wcag21a',
    'wcag21aa',
    'best-practice',
  ])
  for (const sel of opts.exclude ?? []) builder = builder.exclude(sel)
  const result = await builder.analyze()
  const violations = result.violations as AxeViolation[]
  const bad = violations.filter((v) => v.impact === 'serious' || v.impact === 'critical')
  if (violations.length) {
    await testInfo.attach(`axe-${label}`, {
      body: JSON.stringify(
        violations.map((v) => ({
          id: v.id,
          impact: v.impact,
          help: v.help,
          nodes: v.nodes.map((n) => ({ target: n.target, summary: n.failureSummary })),
        })),
        null,
        2,
      ),
      contentType: 'application/json',
    })
  }
  const text = bad
    .map(
      (v) =>
        `${v.impact} ${v.id}: ${v.help}\n` +
        v.nodes
          .slice(0, 4)
          .map((n) => `    ${JSON.stringify(n.target)}  ${n.html.slice(0, 140)}`)
          .join('\n'),
    )
    .join('\n')
  expect(bad, `accessibility (${label}):\n${text}`).toEqual([])
}

/** Presses Tab until `locator` has focus (at most `max` times); a keyboard user's way to reach it. */
export async function tabTo(page: Page, locator: Locator, max = 60): Promise<number> {
  for (let i = 1; i <= max; i++) {
    await page.keyboard.press('Tab')
    const focused = await locator.evaluate((el) => el === document.activeElement).catch(() => false)
    if (focused) return i
  }
  throw new Error(`could not reach the element with Tab in ${max} presses`)
}
