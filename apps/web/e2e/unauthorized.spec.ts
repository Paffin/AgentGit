import { expect, test } from './fixtures.ts'
import { expectAccessible } from './support/ui.ts'

// The page a session that ended while in use lands on (the mid-session 401 itself is in auth.spec.ts, the expiry
// of a token that cannot be renewed in oidc.spec.ts).

test.describe('the unauthorized page', () => {
  test.use({ persona: null })

  test('says so when there is no session, and leads to the sign-in with the way back', async ({
    page,
    cast,
  }) => {
    await page.goto('/unauthorized?next=/repos/acme/webapp')
    await expect(
      page.getByRole('heading', { name: 'You are not signed in', level: 1 }),
    ).toBeVisible()
    await expect(page.getByText('/repos/acme/webapp')).toBeVisible()
    await page.getByTestId('sign-in-again').click()
    await expect(page).toHaveURL(/\/signin\?next=\/repos\/acme\/webapp/)

    // With a session the page has no business being shown.
    await page.getByLabel('Access token').fill(cast.token('erin'))
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.goto('/unauthorized')
    await expect(page).toHaveURL(/127\.0\.0\.1:\d+\/$/)
  })

  test('refuses to lead to another site', async ({ page }) => {
    await page.goto('/unauthorized?next=//evil.example/steal')
    await page.getByTestId('sign-in-again').click()
    await expect(page).toHaveURL(/\/signin$/)
  })

  test('is accessible', async ({ page }, testInfo) => {
    await page.goto('/unauthorized?next=/access')
    await expect(page.getByTestId('sign-in-again')).toBeVisible()
    await expectAccessible(page, testInfo, 'unauthorized')
  })
})
