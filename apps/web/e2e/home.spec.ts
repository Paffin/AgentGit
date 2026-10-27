import { expect, test } from './fixtures.ts'

// Uses the same real server and demo proposals as the review workflow tests.
test('finds work, scopes categories, and recovers from an empty search', async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 1586, height: 992 })
  await page.goto('/')
  const rows = page.getByTestId('attention-item')
  await expect(rows.first()).toBeVisible()
  const initial = await rows.count()
  await page.screenshot({ path: testInfo.outputPath('home-desktop.png') })
  expect(initial).toBeGreaterThan(1)
  const search = page.getByRole('searchbox', { name: 'Search work' })
  await search.fill('invoice')
  await expect(rows).not.toHaveCount(initial)
  await expect(rows.first()).toContainText(/invoice/i)
  await page.getByRole('button', { name: /^Waiting for your review/ }).click()
  await expect(page.getByTestId('attention-failed')).toHaveCount(0)
  await expect(page.getByTestId('attention-review')).toBeVisible()
  await search.fill('no-such-work-849172')
  await expect(page.getByText('No work matches these filters', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Clear filters' }).click()
  await expect(rows).toHaveCount(initial)
  await page.getByTestId('home-refresh').click()
  await expect(page.getByTestId('home-refresh')).toBeEnabled()
  await expect(rows).toHaveCount(initial)
  await expect(page.getByText(/Last refreshed/)).toBeVisible()
})

test('keeps home usable on a narrow screen and in dark mode', async ({ page }, testInfo) => {
  await page.addInitScript(() => localStorage.setItem('agentgit.theme', 'dark'))
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await expect(page.getByTestId('attention-item').first()).toBeVisible()
  await page.getByRole('button', { name: /^Ready to land/ }).click()
  await expect(page.getByTestId('attention-ready')).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('home-phone-dark.png') })
  await expect(page.getByTestId('attention-review')).toHaveCount(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  )
  await page
    .getByRole('banner')
    .getByRole('link', { name: 'All repositories', exact: true })
    .click()
  await expect(page.getByRole('heading', { name: 'Repositories', exact: true })).toBeVisible()
})

test('a failed first load is an error, not an empty work queue, and retry recovers', async ({
  page,
  allowConsole,
}) => {
  allowConsole(/status of 500/)
  let broken = true
  await page.route(/\/v1\/repos\?/, async (route) => {
    if (!broken) return route.continue()
    await route.fulfill({
      status: 500,
      contentType: 'application/json',
      body: JSON.stringify({
        error: {
          code: 'internal',
          message: 'Home summary unavailable',
          request_id: 'home-read-failure',
          retryable: false,
        },
      }),
    })
  })
  await page.goto('/')
  await expect(page.getByText('Could not load your home page', { exact: true })).toBeVisible()
  await expect(page.getByTestId('attention-clear')).toHaveCount(0)
  await expect(page.getByText('Nothing to look at yet', { exact: true })).toHaveCount(0)
  broken = false
  await page.getByRole('button', { name: 'Try again' }).click()
  await expect(page.getByTestId('attention-item').first()).toBeVisible()
})
