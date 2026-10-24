import { expect, test } from './fixtures.ts'

test.describe('attention workspace', () => {
  test('filters a real queue, searches it, and recovers from no matches', async ({
    page,
    state,
  }) => {
    await page.goto('/')
    await expect(page.getByTestId('attention-summary')).toBeVisible()
    const filters = page.getByRole('navigation', { name: 'Filter attention' })
    await filters.getByRole('button', { name: /Ready to land/ }).click()
    await expect(page.getByTestId('attention-ready')).toBeVisible()
    await expect(page.getByTestId('attention-failed')).toHaveCount(0)
    await expect(filters.getByRole('button', { name: /Ready to land/ })).toHaveAttribute(
      'aria-pressed',
      'true',
    )

    await page.getByLabel('Search work').fill('no-such-change-123456')
    await expect(page.getByTestId('attention-no-results')).toBeVisible()
    await page
      .getByTestId('attention-no-results')
      .getByRole('button', { name: 'Clear filters' })
      .click()
    await expect(filters.getByRole('button', { name: /All work/ })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
    await page.getByLabel('Filter by repository').selectOption(state.demo.repo)
    await page.getByLabel('Search work').fill('session store')
    await expect(page.getByTestId('attention-item')).toHaveCount(1)
    await expect(page.getByTestId('attention-item')).toContainText('Extract the session store')
    await expect(page.getByRole('complementary', { name: 'Your repositories' })).toBeVisible()
    await page.getByRole('button', { name: 'Refresh', exact: true }).click()
    await expect(page.getByTestId('attention-item')).toHaveCount(1)
    await expect(page.getByLabel('Search work')).toHaveValue('session store')
  })

  test('keeps queue controls usable on a phone', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/')
    await expect(page.getByTestId('attention-summary')).toBeVisible()
    await page
      .getByRole('navigation', { name: 'Filter attention' })
      .getByRole('button', { name: /Blocked/ })
      .click()
    await expect(page.getByTestId('attention-blocked')).toBeVisible()
    await expect(page.getByLabel('Search work')).toBeVisible()
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true)
  })
})
