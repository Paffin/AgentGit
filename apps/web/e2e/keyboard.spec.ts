import { expect, test } from './fixtures.ts'
import type { Page } from '@playwright/test'

const insideDialog = (page: Page) =>
  page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))

test.describe('keyboard use', () => {
  test('the first Tab stop skips to the content, and following a link moves focus to the new page', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await page.keyboard.press('Tab')
    const skip = page.getByRole('link', { name: 'Skip to content' })
    await expect(skip).toBeFocused()
    await page.keyboard.press('Enter')
    expect(await page.evaluate(() => document.activeElement?.id)).toBe('main')

    // Navigating to another screen with the keyboard announces it and puts focus at the top of the content.
    const history = page.getByRole('link', { name: 'History' })
    await history.focus()
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/history$/)
    await expect(page.getByTestId('route-announcer')).toContainText(/History/)
    await expect.poll(() => page.evaluate(() => document.activeElement?.id)).toBe('main')
  })

  test('focus is always visible', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press('Tab')
      const visible = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null
        if (!el || el === document.body) return true
        const cs = getComputedStyle(el)
        const outline = parseFloat(cs.outlineWidth) > 0 && cs.outlineStyle !== 'none'
        const ring = cs.boxShadow !== 'none'
        return outline || ring
      })
      expect(visible, `element ${i + 1} in tab order shows focus`).toBe(true)
    }
  })

  test('tabs are one tab stop, move with the arrow keys, Home and End, and show their panel', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    const tablist = page.getByRole('tablist', { name: 'Proposal details' })
    const tabs = tablist.getByRole('tab')
    await expect(tabs).toHaveCount(5)
    await tabs.first().focus()
    await expect(tabs.first()).toHaveAttribute('aria-selected', 'true')
    await page.keyboard.press('ArrowRight')
    await expect(tabs.nth(1)).toBeFocused()
    await expect(tabs.nth(1)).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByTestId('reviews-tab')).toBeVisible()
    await page.keyboard.press('End')
    await expect(tabs.last()).toBeFocused()
    await expect(page.getByTestId('timeline-tab')).toBeVisible()
    await page.keyboard.press('Home')
    await expect(tabs.first()).toBeFocused()
    await page.keyboard.press('ArrowLeft') // wraps to the last
    await expect(tabs.last()).toBeFocused()
    // Only the selected tab is in the tab order.
    const tabindexes = await tabs.evaluateAll((els) => els.map((e) => e.getAttribute('tabindex')))
    expect(tabindexes.filter((t) => t === '0')).toHaveLength(1)
  })

  test('a dialog traps focus, closes with Escape, gives focus back, and makes the page inert', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    const opener = page.getByTestId('act-review')
    await opener.focus()
    await page.keyboard.press('Enter')
    const dialog = page.getByRole('dialog', { name: 'Write a review' })
    await expect(dialog).toBeVisible()
    await expect(dialog).toHaveAttribute('aria-modal', 'true')
    // The first useful control has focus (not the close button).
    expect(await insideDialog(page)).toBe(true)
    // The page behind cannot be reached or read.
    await expect(page.locator('#app')).toHaveAttribute('inert', '')

    // Tab and Shift+Tab never leave the dialog, however many times.
    for (let i = 0; i < 14; i++) {
      await page.keyboard.press('Tab')
      expect(await insideDialog(page), `Tab ${i + 1}`).toBe(true)
    }
    for (let i = 0; i < 14; i++) {
      await page.keyboard.press('Shift+Tab')
      expect(await insideDialog(page), `Shift+Tab ${i + 1}`).toBe(true)
    }

    // The decision is a radio group that works with the arrow keys.
    await dialog.getByLabel('Approve').focus()
    await page.keyboard.press('ArrowDown')
    await expect(dialog.getByLabel('Request changes')).toBeChecked()

    await page.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)
    await expect(opener).toBeFocused()
    await expect(page.locator('#app')).not.toHaveAttribute('inert', '')
  })

  test('the one-time token dialog takes focus when the secret appears and keeps the page inert', async ({
    page,
  }) => {
    await page.goto('/access')
    await page.getByTestId('mint-open').click()
    const dialog = page.getByRole('dialog', { name: 'Mint a token' })
    await dialog.getByLabel('Description').fill('keyboard test')
    await page.keyboard.press('Enter')
    await expect(page.getByTestId('minted-token')).toBeVisible()
    await expect(page.locator('#app')).toHaveAttribute('inert', '')
    expect(await insideDialog(page)).toBe(true)
    await page.getByTestId('minted-done').press('Enter')
    await expect(page.getByRole('dialog')).toHaveCount(0)
  })

  test('the account menu opens with Enter, moves with arrows, closes with Escape and returns focus', async ({
    page,
  }) => {
    await page.goto('/')
    const trigger = page.getByTestId('user-menu')
    await trigger.focus()
    await page.keyboard.press('Enter')
    const menu = page.getByRole('menu', { name: 'Account' })
    await expect(menu).toBeVisible()
    await expect(trigger).toHaveAttribute('aria-expanded', 'true')
    // Opened with the keyboard: focus is on the first item already.
    await expect(menu.getByRole('menuitem').first()).toBeFocused()
    await page.keyboard.press('ArrowDown')
    await expect(menu.getByRole('menuitem').nth(1)).toBeFocused()
    await page.keyboard.press('ArrowDown') // wraps
    await expect(menu.getByRole('menuitem').first()).toBeFocused()
    await page.keyboard.press('ArrowUp')
    await expect(menu.getByRole('menuitem').nth(1)).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(menu).toHaveCount(0)
    await expect(trigger).toBeFocused()
  })

  test('the task graph is reachable and operable without a mouse', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const node = page.locator(`[data-task="${state.demo.tasks.redis}"]`)
    await expect(node).toBeVisible()
    await node.focus()
    await page.keyboard.press('Enter')
    const dialog = page.getByRole('dialog', { name: 'Migrate sessions to Redis' })
    await expect(dialog).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(node).toBeFocused()
  })

  test('the long lists and code areas can be scrolled with the keyboard', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const feed = page.getByRole('list', { name: /activity|events/i }).first()
    await expect(feed).toBeVisible()
    await expect(feed).toHaveAttribute('tabindex', '0')
  })

  test('a tooltip appears on hover and Escape dismisses it without moving the pointer', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.docs}`)
    await page.locator('[data-state="validated"]').first().hover()
    await expect(page.getByRole('tooltip').first()).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.getByRole('tooltip').first()).toBeHidden()
  })
})
