import { expect, test, TOKEN_KEY } from './fixtures.ts'

test.describe('signing in', () => {
  test.use({ persona: null })

  test('a bad token is refused with a plain explanation and nothing is stored', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 401/)
    await page.goto('/')
    await expect(page).toHaveURL(/\/signin$/)
    await page.getByLabel('Access token').fill('agt_not_a_real_token_0123456789')
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()

    const alert = page.getByRole('alert').filter({ hasText: 'Could not sign in' })
    await expect(alert).toContainText('The server did not accept that token')
    await expect(page).toHaveURL(/\/signin$/)
    expect(await page.evaluate((k) => sessionStorage.getItem(k), TOKEN_KEY)).toBeNull()
    // The token field is still there for another try, and the secret was not echoed into the page.
    await expect(page.getByLabel('Access token')).toBeVisible()
    await expect(page.getByRole('alert')).not.toContainText('agt_not_a_real_token')
  })

  test('a good token shows who you are and what you may do, then opens the repositories', async ({
    page,
    cast,
  }) => {
    await page.goto('/signin')
    await page.getByLabel('Access token').fill(cast.token('bob'))
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()

    const card = page.getByRole('heading', { name: 'You are signed in' })
    await expect(card).toBeVisible()
    const who = page.getByRole('region', { name: 'You are signed in' })
    await expect(who.getByText('user:bob').first()).toBeVisible()
    await expect(who.getByText('acme/*:reviewer')).toBeVisible()
    await expect(who.getByText('Scoped token')).toBeVisible()
    // The password field was cleared once it was no longer needed.
    await expect(page.getByLabel('Access token')).toHaveCount(0)

    await page.getByRole('button', { name: 'Continue' }).click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
  })

  test('the token lives in sessionStorage only and never in a URL, and survives a reload', async ({
    page,
    cast,
  }) => {
    const token = cast.token('alice')
    await page.goto('/signin')
    await page.getByLabel('Access token').fill(token)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()

    const stores = await page.evaluate((k) => {
      const dump = (s: Storage) => Object.fromEntries(Object.entries(s))
      return { session: dump(sessionStorage), local: dump(localStorage), url: location.href }
    }, TOKEN_KEY)
    expect(stores.session[TOKEN_KEY]).toBe(token)
    expect(JSON.stringify(stores.local)).not.toContain(token)
    expect(stores.url).not.toContain(token)

    await page.reload()
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
    await expect(page.getByTestId('user-menu')).toContainText('user:alice')
  })

  test('the token is only ever sent to the server in an Authorization header', async ({
    page,
    cast,
  }) => {
    const token = cast.token('alice')
    const leaks: string[] = []
    page.on('request', (req) => {
      if (req.url().includes(token)) leaks.push(`url: ${req.url()}`)
      if (req.postData()?.includes(token)) leaks.push(`body: ${req.url()}`)
      const auth = req.headers()['authorization']
      if (auth && auth !== `Bearer ${token}`) leaks.push(`unexpected authorization on ${req.url()}`)
    })
    await page.goto('/signin')
    await page.getByLabel('Access token').fill(token)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.getByRole('link', { name: 'acme/webapp' }).first().click()
    await expect(page.getByTestId('board-summary')).toBeVisible()
    expect(leaks).toEqual([])
  })

  test('explains the ways to sign in, including single sign-on', async ({ page }) => {
    await page.goto('/signin')
    await expect(page.getByRole('heading', { name: 'Which token do I use?' })).toBeVisible()
    await expect(page.getByText('Single sign-on (OIDC).')).toBeVisible()
    await expect(page.getByText(/ID token/)).toBeVisible()
    await expect(page.getByText(/protocol agwp\.v1/)).toBeVisible()
  })

  test('asks you to sign in first and then returns to where you were going', async ({
    page,
    cast,
    state,
  }) => {
    const target = `/repos/acme/webapp/proposals/${state.demo.proposals.docs}`
    await page.goto(target)
    await expect(page).toHaveURL(/\/signin\?next=/)
    await page.getByLabel('Access token').fill(cast.token('erin'))
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await expect(page).toHaveURL(new RegExp(`${target}$`))
    await expect(page.getByTestId('proposal-title')).toContainText('Document how sessions')
  })

  test('refuses to bounce to another site after signing in', async ({ page, cast }) => {
    await page.goto('/signin?next=//evil.example/steal')
    await page.getByLabel('Access token').fill(cast.token('erin'))
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await expect(page).toHaveURL(/127\.0\.0\.1:\d+\/$/)
  })

  test('signing out ends the session and clears the token', async ({ page, cast }) => {
    await page.goto('/signin')
    await page.getByLabel('Access token').fill(cast.token('carol'))
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.getByTestId('user-menu').click()
    await page.getByRole('menuitem', { name: /sign out/i }).click()
    await expect(page).toHaveURL(/\/signin/)
    expect(await page.evaluate((k) => sessionStorage.getItem(k), TOKEN_KEY)).toBeNull()
    await page.goto('/repos/acme/webapp')
    await expect(page).toHaveURL(/\/signin\?next=/)
  })

  test('a credential revoked while you work ends the session with an explanation', async ({
    page,
    cast,
    allowConsole,
  }) => {
    allowConsole(/status of 401/)
    const admin = cast.as('admin')
    const minted = await admin.ok_(
      admin.post('/v1/credentials', {
        subject: 'user:alice',
        description: 'revoked mid-session',
        ttl_secs: 600,
      }),
      'mint',
    )
    await page.goto('/signin')
    await page.getByLabel('Access token').fill(minted.token)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue' }).click()
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()

    await admin.ok_(admin.del(`/v1/credentials/${minted.credential.id}`), 'revoke')
    // Follow a link with the keyboard: the very next request the app makes is answered with 401.
    await page.getByRole('link', { name: 'acme/webapp' }).first().focus()
    await page.keyboard.press('Enter')

    await expect(page).toHaveURL(/\/signin/)
    await expect(page.getByText(/Your session ended: the credential expired or was revoked/)).toBeVisible()
    expect(await page.evaluate((k) => sessionStorage.getItem(k), TOKEN_KEY)).toBeNull()
  })
})

test('a saved token that stopped working sends you back to sign in', async ({
  browser,
  baseURL,
  allowConsole,
}) => {
  allowConsole(/status of 401/)
  const ctx = await browser.newContext({ baseURL })
  const page = await ctx.newPage()
  await page.addInitScript((k) => sessionStorage.setItem(k, 'agt_revoked_0000_0000_0000'), TOKEN_KEY)
  await page.goto('/')
  await expect(page).toHaveURL(/\/signin/)
  await expect(page.getByText(/no longer valid/)).toBeVisible()
  await ctx.close()
})
