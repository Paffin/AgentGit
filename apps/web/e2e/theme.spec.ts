import type { Page } from '@playwright/test'
import { expect, test } from './fixtures.ts'

const luminance = (rgb: string): number => {
  const m = rgb.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0]
  const [r = 0, g = 0, b = 0] = m.map((v) => {
    const c = v / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

async function colours(page: Page) {
  return page.evaluate(() => ({
    theme: document.documentElement.getAttribute('data-theme'),
    body: getComputedStyle(document.body).backgroundColor,
    text: getComputedStyle(document.body).color,
    card: getComputedStyle(document.querySelector('.card, [class*="card"]') ?? document.body)
      .backgroundColor,
  }))
}

test.describe('themes', () => {
  test.describe('following the system', () => {
    test.use({ colorScheme: 'dark' })
    test('a dark system gets the dark theme without asking', async ({ page, state }) => {
      await page.goto(`/repos/${state.demo.repo}`)
      await expect(page.getByTestId('board-summary')).toBeVisible()
      const c = await colours(page)
      expect(c.theme).toBeNull()
      expect(luminance(c.body)).toBeLessThan(0.05)
      expect(luminance(c.text)).toBeGreaterThan(0.5)
      expect(
        await page.evaluate(() => getComputedStyle(document.documentElement).colorScheme),
      ).toContain('dark')
    })
  })

  test.describe('light system', () => {
    test.use({ colorScheme: 'light' })

    test('the toggle cycles system, light, dark and remembers the choice', async ({
      page,
      state,
    }) => {
      await page.goto(`/repos/${state.demo.repo}`)
      await expect(page.getByTestId('board-summary')).toBeVisible()
      const toggle = page.getByTestId('theme-toggle')
      await expect(toggle).toHaveAccessibleName(/follow system/i)
      expect(luminance((await colours(page)).body)).toBeGreaterThan(0.5)

      await toggle.click() // system -> light
      await expect(toggle).toHaveAccessibleName(/light/i)
      expect((await colours(page)).theme).toBe('light')

      await toggle.click() // light -> dark, even though the system is light
      await expect(toggle).toHaveAccessibleName(/dark/i)
      const dark = await colours(page)
      expect(dark.theme).toBe('dark')
      expect(luminance(dark.body)).toBeLessThan(0.05)
      expect(luminance(dark.text)).toBeGreaterThan(0.5)

      // It is remembered for this browser (and only a preference: no token, no data).
      await page.reload()
      await expect(page.getByTestId('board-summary')).toBeVisible()
      expect((await colours(page)).theme).toBe('dark')
      expect(await page.evaluate(() => localStorage.getItem('agentgit.theme'))).toBe('dark')

      await toggle.click() // dark -> system
      expect((await colours(page)).theme).toBeNull()
      expect(luminance((await colours(page)).body)).toBeGreaterThan(0.5)
    })

    test('the dark theme reaches every surface: cards, tables, code, diffs, dialogs, tooltips', async ({
      page,
      state,
    }) => {
      await page.addInitScript(() => localStorage.setItem('agentgit.theme', 'dark'))
      await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
      await expect(page.getByTestId('changes-tab')).toContainText('3 files changed')
      const surfaces = await page.evaluate(() => {
        const bg = (sel: string) => {
          const el = document.querySelector(sel)
          return el ? getComputedStyle(el).backgroundColor : null
        }
        return {
          body: bg('body'),
          diffRow: bg('.row.k-eq') ?? bg('.diff'),
          diffAdd: bg('.row.k-add'),
          diffDel: bg('.row.k-del'),
          header: bg('header'),
        }
      })
      for (const [name, value] of Object.entries(surfaces)) {
        if (!value) continue
        expect(luminance(value), `${name} is dark`).toBeLessThan(0.2)
      }
      await page.getByTestId('act-review').click()
      const dialog = page.getByRole('dialog', { name: 'Write a review' })
      const dialogBg = await dialog.evaluate((el) => getComputedStyle(el).backgroundColor)
      expect(luminance(dialogBg)).toBeLessThan(0.2)
    })

    test('added and removed lines are told apart by more than colour', async ({ page, state }) => {
      await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
      const add = page.locator('.row.k-add').first()
      const del = page.locator('.row.k-del').first()
      await expect(add).toContainText('added:')
      await expect(del).toContainText('removed:')
      await expect(add.locator('.sg')).toHaveText('+')
      await expect(del.locator('.sg')).toHaveText('−')
    })
  })
})
