import { expect, test } from './fixtures.ts'
import type { Page } from '@playwright/test'

// What the UI does when things are missing, forbidden, slow, broken or unreachable.

test.describe('missing, forbidden, broken', () => {
  test('an unknown page says so and leads back', async ({ page }) => {
    await page.goto('/nowhere/at/all')
    await expect(page.getByText('This page does not exist')).toBeVisible()
    await page
      .getByRole('link', { name: /repositories/i })
      .first()
      .click()
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
  })

  test('an unknown repository or proposal is "not found", not a blank page', async ({
    page,
    state,
    allowConsole,
  }) => {
    allowConsole(/status of 404/)
    await page.goto('/repos/acme/does-not-exist')
    await expect(page.getByText(/not found|does not exist|no such/i).first()).toBeVisible()
    await page.goto(`/repos/${state.demo.repo}/proposals/prop_000000000000000000`)
    await expect(page.getByText(/not found|does not exist|no such/i).first()).toBeVisible()
  })

  test('a credential that may not see a repository gets the forbidden screen, with the reason', async ({
    page,
    cast,
    allowConsole,
  }) => {
    allowConsole(/status of 40[34]/)
    const admin = cast.as('admin')
    const minted = await admin.ok_(
      admin.post('/v1/credentials', {
        subject: 'user:alice',
        ttl_secs: 600,
        scope: { repos: ['acme/mobile'] },
      }),
      'mint a narrow token',
    )
    await page.addInitScript((t) => sessionStorage.setItem('agentgit.token', t), minted.token)
    await page.goto('/repos/acme/webapp')
    await expect(page.getByText('You do not have access', { exact: true })).toBeVisible()
    await expect(page.getByText(/not scoped for 'repo.read'/)).toBeVisible()
    await expect(page.getByRole('link', { name: /repositories/i }).first()).toBeVisible()
  })
})

async function slow(page: Page, pattern: RegExp, ms: number) {
  await page.route(pattern, async (route) => {
    await new Promise((r) => setTimeout(r, ms))
    await route.continue()
  })
}

test.describe('loading', () => {
  test('a slow list shows a skeleton of the right shape, then the data, without the page jumping', async ({
    page,
  }) => {
    await page.addInitScript(() => {
      const w = window as unknown as { __cls: number }
      w.__cls = 0
      // Layout-shift entries exist only in Chromium (Firefox and WebKit warn "unsupported entryTypes"): there the
      // observer is not installed and the measure stays 0, so these two tests check the rest of their story only.
      if (!PerformanceObserver.supportedEntryTypes?.includes('layout-shift')) return
      new PerformanceObserver((list) => {
        for (const e of list.getEntries() as unknown as {
          value: number
          hadRecentInput: boolean
        }[])
          if (!e.hadRecentInput) w.__cls += e.value
      }).observe({ type: 'layout-shift', buffered: true })
    })
    await slow(page, /\/v1\/repos\?/, 1200)
    await page.goto('/repos')
    const table = page.getByRole('table', { name: 'Repositories' })
    await expect(table).toHaveAttribute('aria-busy', 'true')
    await expect(page.getByRole('link', { name: 'acme/webapp' })).toBeVisible({ timeout: 15_000 })
    await expect(table).not.toHaveAttribute('aria-busy', 'true')
    await page.waitForTimeout(500)
    const cls = await page.evaluate(() => (window as unknown as { __cls: number }).__cls)
    expect(cls, 'cumulative layout shift').toBeLessThan(0.1)
  })

  test('the live board settles without shifting its layout', async ({ page, state }) => {
    await page.addInitScript(() => {
      const w = window as unknown as { __cls: number }
      w.__cls = 0
      // Layout-shift entries exist only in Chromium (Firefox and WebKit warn "unsupported entryTypes"): there the
      // observer is not installed and the measure stays 0, so these two tests check the rest of their story only.
      if (!PerformanceObserver.supportedEntryTypes?.includes('layout-shift')) return
      new PerformanceObserver((list) => {
        for (const e of list.getEntries() as unknown as {
          value: number
          hadRecentInput: boolean
        }[])
          if (!e.hadRecentInput) w.__cls += e.value
      }).observe({ type: 'layout-shift', buffered: true })
    })
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('proposals-panel').getByText('checking')).toHaveCount(0, {
      timeout: 20_000,
    })
    await page.waitForTimeout(1500)
    const cls = await page.evaluate(() => (window as unknown as { __cls: number }).__cls)
    expect(cls, 'cumulative layout shift').toBeLessThan(0.1)
  })
})

test.describe('trouble', () => {
  test('a server error is shown with its request id, and Retry works once it is fixed', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 500/)
    let broken = true
    await page.route(/\/v1\/repos\?/, async (route) => {
      if (broken) {
        await route.fulfill({
          status: 500,
          contentType: 'application/json',
          body: JSON.stringify({
            error: {
              code: 'internal',
              message: 'the database is on fire',
              request_id: 'req_e2e500',
              retryable: false,
            },
          }),
        })
      } else await route.continue()
    })
    await page.goto('/repos')
    await expect(page.getByText('the database is on fire')).toBeVisible()
    await expect(page.getByText('req_e2e500')).toBeVisible()
    broken = false
    await page.getByRole('button', { name: /retry|try again/i }).click()
    await expect(page.getByRole('link', { name: 'acme/webapp' })).toBeVisible()
  })

  test('rate limiting is waited out transparently', async ({ page, allowConsole }) => {
    allowConsole(/status of 429/)
    let first = true
    await page.route(/\/v1\/repos\?/, async (route) => {
      if (first) {
        first = false
        await route.fulfill({
          status: 429,
          headers: { 'retry-after': '1' },
          contentType: 'application/json',
          body: JSON.stringify({
            error: {
              code: 'rate_limited',
              message: 'slow down',
              request_id: 'req_e2e429',
              retryable: true,
              retry_after_ms: 1000,
            },
          }),
        })
      } else await route.continue()
    })
    await page.goto('/repos')
    // No error is ever shown: the client retried after the advertised delay.
    await expect(page.getByRole('link', { name: 'acme/webapp' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('slow down')).toHaveCount(0)
  })

  test('an unreachable server shows the offline screen instead of a sign-in form, and recovers', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/ERR_CONNECTION_REFUSED|ERR_FAILED|ERR_INTERNET_DISCONNECTED/)
    const whoami = /\/v1\/whoami/
    await page.route(whoami, (route) => route.abort('connectionrefused'))
    await page.goto('/repos/acme/webapp')
    await expect(page).toHaveURL(/\/offline/)
    await expect(page.getByText('Cannot reach the server')).toBeVisible()
    await page.unroute(whoami)
    await page.getByRole('button', { name: /try again|retry/i }).click()
    await expect(page.getByTestId('board-summary')).toBeVisible({ timeout: 20_000 })
  })
})
