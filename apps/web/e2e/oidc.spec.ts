import type { Page } from '@playwright/test'
import { expect, test } from './fixtures.ts'
import type { Scenario } from './support/idp.ts'
import { suffix } from './support/world.ts'

// Redirect sign-in (OpenID Connect authorization code with PKCE) against a REAL ag-server, which trusts a small
// fake identity provider (e2e/support/idp.ts) that plays the part honestly: it insists on the registered redirect
// URI and on PKCE, and it can be told to go wrong. The server validates every token it is given with the
// provider's published key; nothing about that is mocked.

test.use({ persona: null })

const user = () => `sso-${suffix()}`

/** Everything the provider recorded for this user. */
async function idpEvents(page: Page, issuer: string, name: string) {
  const res = await page.request.get(`${issuer}/__test/events?user=${encodeURIComponent(name)}`)
  return (await res.json()) as { kind: string; user?: string; detail: Record<string, unknown> }[]
}

/** On the sign-in page: press the provider's button, then play the user at the provider's login page. */
async function signInAtProvider(page: Page, name: string, scenario: Scenario = 'normal') {
  await page.getByRole('button', { name: 'Sign in with Fake SSO' }).click()
  await expect(page.getByRole('heading', { name: 'Fake identity provider' })).toBeVisible()
  await page.getByLabel('Username').fill(name)
  await page.getByLabel('What should happen').selectOption(scenario)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
}

test.describe('redirect sign-in', () => {
  test('the sign-in page offers the provider and still takes a pasted token', async ({ page }) => {
    await page.goto('/signin')
    await expect(page.getByRole('button', { name: 'Sign in with Fake SSO' })).toBeVisible()
    await expect(page.getByLabel('Access token')).toBeVisible()
  })

  test('the round trip: PKCE out, a code back, tokens in memory only, then where you were going', async ({
    page,
    state,
    baseURL,
  }) => {
    const name = user()
    const bearers: string[] = []
    page.on('request', (r) => {
      const auth = r.headers()['authorization']
      if (auth && r.url().startsWith(baseURL ?? '')) bearers.push(auth)
    })

    // A deep link while signed out leads to the sign-in page, remembering the destination.
    await page.goto(`/repos/${state.demo.repo}/proposals`)
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    await page.getByRole('button', { name: 'Sign in with Fake SSO' }).click()

    // What the browser sent to the provider.
    await expect(page.getByRole('heading', { name: 'Fake identity provider' })).toBeVisible()
    const asked = new URL(page.url())
    expect(asked.origin).toBe(state.idp.issuer)
    const q = asked.searchParams
    expect(q.get('response_type')).toBe('code')
    expect(q.get('client_id')).toBe(state.idp.clientId)
    expect(q.get('redirect_uri')).toBe(`${baseURL}/auth/callback`)
    expect(q.get('code_challenge_method')).toBe('S256')
    expect(q.get('code_challenge')).toMatch(/^[A-Za-z0-9_-]{43}$/)
    expect(q.get('state')).toMatch(/^[A-Za-z0-9_-]{20,}$/)
    expect(q.get('nonce')).toMatch(/^[A-Za-z0-9_-]{20,}$/)
    expect(q.get('scope')?.split(' ')).toContain('openid')
    expect(q.has('client_secret')).toBe(false)
    expect(q.has('code_verifier')).toBe(false)
    expect(q.has('prompt')).toBe(false)

    await page.getByLabel('Username').fill(name)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()

    // Back in the app, signed in as the identity the server derived from the token, at the page we asked for.
    await expect(page).toHaveURL(`${baseURL}/repos/${state.demo.repo}/proposals`)
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)
    await expect(page.getByRole('heading', { name: 'Change requests', level: 1 })).toBeVisible()
    // The `staff` group maps to a read-only role on acme/*, and that is all the server gave this person.
    await expect(page.getByTestId('user-menu')).toBeVisible()

    // The one-time code and state are not left in the address bar, and no token is left in any storage.
    expect(page.url()).not.toMatch(/[?&](code|state)=/)
    const stored = await page.evaluate(() =>
      JSON.stringify({ session: { ...sessionStorage }, local: { ...localStorage } }),
    )
    expect(stored).not.toMatch(/eyJ[A-Za-z0-9_-]{10,}/)
    expect(stored).not.toContain('agentgit.token')
    expect(stored).not.toContain('agentgit.oidc.flow')
    expect(stored).not.toContain('code_verifier')
    // ... yet the server was spoken to with a real JWT.
    expect(bearers.some((b) => /^Bearer eyJ/.test(b))).toBe(true)

    // What the provider saw: PKCE verified, exactly one code exchange.
    const events = await idpEvents(page, state.idp.issuer, name)
    expect(events.filter((e) => e.kind === 'token')).toHaveLength(1)
    expect(events.find((e) => e.kind === 'token')?.detail.pkce).toBe('verified')

    // The server says who this is and how: group `staff` made them a reader of acme/*.
    await page.getByTestId('user-menu').click()
    await page.getByRole('menuitem', { name: 'Credentials and roles' }).click()
    await expect(page.getByText('OIDC single sign-on').first()).toBeVisible()
    await expect(page.getByText(/acme\/\*.*reader|reader.*acme\/\*/).first()).toBeVisible()
  })

  test('a reload asks the provider again, silently, and the token is never stored', async ({
    page,
    state,
    baseURL,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name)
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)

    await page.goto(`${baseURL}/repos/${state.demo.repo}/streams`)
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)
    await expect(page).toHaveURL(`${baseURL}/repos/${state.demo.repo}/streams`)

    const events = await idpEvents(page, state.idp.issuer, name)
    const silent = events.filter((e) => e.kind === 'authorize' && e.detail.prompt === 'none')
    expect(silent.length).toBeGreaterThanOrEqual(1)
    expect(silent.every((e) => e.detail.hasSession === true)).toBe(true)
    expect(events.filter((e) => e.kind === 'login')).toHaveLength(1) // typed credentials once, not again
  })

  test('without the provider’s own session, a reload ends at the sign-in page and says why', async ({
    page,
    context,
    state,
    baseURL,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name)
    await expect(page.getByTestId('user-menu')).toBeVisible()

    // The provider's session ends elsewhere (cookie gone): the silent request is answered `login_required`.
    await context.clearCookies()
    await page.goto(`${baseURL}/repos/${state.demo.repo}`)
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    await expect(page.getByText('Your single sign-on session has ended')).toBeVisible()
    // ... and the page does not bounce between the two sites.
    await expect(page.getByRole('button', { name: 'Sign in with Fake SSO' })).toBeVisible()
  })

  test('signing out ends the provider’s session too, and the next sign-in asks for credentials again', async ({
    page,
    state,
    baseURL,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name)
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)

    await page.getByTestId('user-menu').click()
    await page.getByRole('menuitem', { name: 'Sign out' }).click()
    await expect(page).toHaveURL(`${baseURL}/signin`)
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    const logout = (await idpEvents(page, state.idp.issuer, name)).filter(
      (e) => e.kind === 'logout',
    )
    expect(logout).toHaveLength(1)
    expect(logout[0]?.detail.idTokenHint).toBe(true)
    expect(logout[0]?.detail.hadSession).toBe(true)

    // The app forgot everything: a protected page leads back to sign-in instead of resuming.
    await page.goto(`${baseURL}/repos/${state.demo.repo}`)
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    // ... and the provider no longer remembers the person: it shows its login page, not a silent success.
    await page.getByRole('button', { name: 'Sign in with Fake SSO' }).click()
    await expect(page.getByRole('heading', { name: 'Fake identity provider' })).toBeVisible()
  })
})

test.describe('redirect sign-in that goes wrong', () => {
  test('a token that has already expired is refused before it is used', async ({ page, state }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name, 'expired-token')
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    await expect(page.getByRole('alert')).toContainText('Single sign-on did not complete')
    await expect(page.getByRole('alert')).toContainText('already expired')
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
    // Nothing is left over from the attempt.
    const stored = await page.evaluate(() => JSON.stringify({ ...sessionStorage }))
    expect(stored).not.toMatch(/eyJ/)
    expect(stored).not.toContain('agentgit.oidc.flow')
    expect(
      (await idpEvents(page, state.idp.issuer, name)).filter((e) => e.kind === 'token'),
    ).toHaveLength(1)
  })

  test('a response with the wrong state is ignored without asking for tokens', async ({
    page,
    state,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name, 'wrong-state')
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    await expect(page.getByRole('alert')).toContainText('did not match the request')
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
    // The code was never redeemed: the forged response cost the attacker nothing and gained nothing.
    const events = await idpEvents(page, state.idp.issuer, name)
    expect(events.filter((e) => e.kind === 'token')).toHaveLength(0)
  })

  test('an ID token for another nonce is refused', async ({ page }) => {
    await page.goto('/signin')
    await signInAtProvider(page, user(), 'bad-nonce')
    await expect(page.getByRole('alert')).toContainText('nonce')
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
  })

  test('a user who says no is told so', async ({ page }) => {
    await page.goto('/signin')
    await signInAtProvider(page, user(), 'denied')
    await expect(page.getByRole('alert')).toContainText('refused the sign-in')
    await expect(page.getByRole('alert')).toContainText('The user said no')
    await expect(page.getByRole('button', { name: 'Sign in with Fake SSO' })).toBeVisible()
  })

  test('a token the server does not trust is explained, not shown as a blank page', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 401/)
    await page.goto('/signin')
    await signInAtProvider(page, user(), 'bad-signature')
    await expect(page.getByRole('alert')).toContainText('did not accept the ID token')
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
  })

  test('a callback that nobody asked for says there is no sign-in in progress', async ({
    page,
  }) => {
    await page.goto('/auth/callback?code=stolen&state=forged')
    await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
    await expect(page.getByRole('alert')).toContainText('no sign-in in progress')
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
  })
})

test.describe('sessions that run out', () => {
  test('a token that is about to expire is renewed silently with the refresh token', async ({
    page,
    state,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name, 'short-renewable')
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)
    // The first token lives five seconds; the UI trades the refresh token for a new one before it ends.
    await expect
      .poll(
        async () =>
          (await idpEvents(page, state.idp.issuer, name)).filter((e) => e.kind === 'refresh')
            .length,
        {
          timeout: 15_000,
        },
      )
      .toBeGreaterThanOrEqual(1)
    // Well past the first token's life, the session still works and nobody was asked anything.
    await page.waitForTimeout(6_000)
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)
    await expect(page.getByText('Your credential is about to expire')).toHaveCount(0)
    await page.getByRole('link', { name: 'Repositories' }).first().click()
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
    await expect(page.getByRole('link', { name: state.demo.repo })).toBeVisible()
  })

  test('a token that cannot be renewed warns, then ends the session with an explanation', async ({
    page,
    state,
  }) => {
    const name = user()
    await page.goto('/signin')
    await signInAtProvider(page, name, 'short-no-refresh')
    await expect(page.getByTestId('user-menu')).toContainText(`user:${name}`)
    // Not renewable: the person is warned, and can sign in again from the banner.
    await expect(page.getByText('Your credential is about to expire')).toBeVisible()
    await expect(page.getByTestId('reauth')).toBeVisible()
    // ... and if they do not, the session ends when the token does.
    await expect(page.getByRole('heading', { name: 'Your session ended', level: 1 })).toBeVisible({
      timeout: 20_000,
    })
    await expect(page).toHaveURL(/\/unauthorized/)
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
    expect(
      (await idpEvents(page, state.idp.issuer, name)).filter((e) => e.kind === 'refresh'),
    ).toHaveLength(0)
  })
})
