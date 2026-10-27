import { expect, test } from './fixtures.ts'
import { screens } from './support/screens.ts'
import { expectAccessible } from './support/ui.ts'

// An automated accessibility audit (axe-core, WCAG 2.x A and AA plus best practices) of every main screen,
// in both themes. Serious and critical findings fail the test. Automated checks find only part of the
// problems: keyboard use and focus handling are tested separately (keyboard.spec.ts).

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`axe, ${scheme} theme`, () => {
    test.use({ colorScheme: scheme })

    for (const screen of screens) {
      test(`${screen.name} has no serious or critical violations`, async ({
        page,
        state,
        openAs,
      }, testInfo) => {
        const target = screen.persona ? await openAs(screen.persona, { colorScheme: scheme }) : page
        await target.goto(screen.path(state))
        await screen.ready(target)
        // The follow-the-system theme really followed the system.
        const dark = await target.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches)
        expect(dark).toBe(scheme === 'dark')
        await expectAccessible(target, testInfo, `${screen.name} (${scheme})`)
      })
    }
  })
}

test.describe('axe, signed-out and error screens', () => {
  test.use({ persona: null })

  test('sign in', async ({ page }, testInfo) => {
    await page.goto('/signin')
    await expect(page.getByLabel('Access token')).toBeVisible()
    await expectAccessible(page, testInfo, 'sign in')
  })

  test('sign in with an error showing', async ({ page, allowConsole }, testInfo) => {
    allowConsole(/status of 401/)
    await page.goto('/signin')
    await page.getByLabel('Access token').fill('agt_bad_0000')
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await expect(page.getByRole('alert').filter({ hasText: 'Could not sign in' })).toBeVisible()
    await expectAccessible(page, testInfo, 'sign in error')
  })

  test('not found', async ({ page }, testInfo) => {
    await page.goto('/nowhere/at/all')
    await expect(page.getByText('This page does not exist')).toBeVisible()
    await expectAccessible(page, testInfo, 'not found')
  })

  test('offline', async ({ page }, testInfo) => {
    await page.goto('/offline')
    await expect(page.getByText('Cannot reach the server')).toBeVisible()
    await expectAccessible(page, testInfo, 'offline')
  })
})

test.describe('axe, dialogs', () => {
  test('the review, integrate and mint dialogs', async ({ page, state }, testInfo) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    await page.getByTestId('act-review').click()
    await expect(page.getByRole('dialog', { name: 'Write a review' })).toBeVisible()
    await expectAccessible(page, testInfo, 'review dialog')
    await page.keyboard.press('Escape')
    await page.getByTestId('act-integrate').click()
    await expect(page.getByRole('dialog', { name: 'Land this change request' })).toBeVisible()
    await expectAccessible(page, testInfo, 'integrate dialog')
    await page.keyboard.press('Escape')
    await page.goto('/access')
    await page.getByTestId('mint-open').click()
    await expect(page.getByRole('dialog', { name: 'Mint a token' })).toBeVisible()
    await expectAccessible(page, testInfo, 'mint dialog')
  })
})
