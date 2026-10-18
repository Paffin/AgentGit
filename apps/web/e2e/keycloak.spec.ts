import type { Page } from '@playwright/test'
import { expect, test } from '@playwright/test'
import { resolve } from 'node:path'
import { defaultUi, startServer } from './support/server.ts'
import type { RunningServer } from './support/server.ts'
import { Api } from './support/http.ts'

// Redirect sign-in against a REAL identity provider: Keycloak, the realm in e2e/keycloak/agentgit-realm.json,
// started by the "real identity provider" job of .github/workflows/web.yml (a pinned image). A real ag-server
// trusts it through discovery (no key file, no test shortcut) and validates every token with the keys Keycloak
// publishes. What is covered here and in oidc.spec.ts (the fast fake provider, a test double that can also go
// wrong on purpose, which Keycloak cannot): the login round trip, the groups-to-roles mapping, silent
// re-authentication after a reload, token expiry once the provider's session is gone, and sign-out at the
// provider.
//
// Needs AG_E2E_KEYCLOAK (the base URL of Keycloak, `http://localhost:8180`); other jobs skip it. Locally:
//   docker run --rm -p 8180:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
//     -v $PWD/e2e/keycloak:/opt/keycloak/data/import quay.io/keycloak/keycloak:26.0.7 start-dev --import-realm

const KEYCLOAK = process.env.AG_E2E_KEYCLOAK?.replace(/\/+$/, '') ?? ''
const REALM = 'agentgit'
const ISSUER = `${KEYCLOAK}/realms/${REALM}`
const CLIENT = 'agentgit-web'

test.skip(
  !KEYCLOAK,
  'needs a Keycloak: set AG_E2E_KEYCLOAK (the "real identity provider" CI job does)',
)
// One Keycloak, a few tests that change its state: they run one after the other.
test.describe.configure({ mode: 'serial' })

let server: RunningServer

test.beforeAll(async () => {
  if (!KEYCLOAK) return
  server = await startServer({
    ui: defaultUi(resolve(import.meta.dirname, '../dist')),
    extraConfig: [
      '[[auth.oidc]]',
      `issuer = ${JSON.stringify(ISSUER)}`,
      `audience = ${JSON.stringify(CLIENT)}`,
      'algorithms = ["RS256"]',
      'group_roles = [',
      '  { group = "staff", role = "reader", repo = "acme/*" },',
      '  { group = "platform", role = "admin", repo = "acme/*" },',
      ']',
      '',
      '[[ui.login]]',
      `issuer = ${JSON.stringify(ISSUER)}`,
      `client_id = ${JSON.stringify(CLIENT)}`,
      'name = "Keycloak"',
      '',
    ],
  })
  const admin = new Api(server.baseURL, server.adminToken)
  await admin.ok_(admin.post('/v1/repos', { name: 'acme/webapp' }), 'create the repository')
})
test.afterAll(async () => server?.stop())

/** On the sign-in page: press the provider's button, then type the credentials into Keycloak's own login page. */
async function signInAtKeycloak(page: Page, username: string, password: string) {
  await page.getByRole('button', { name: 'Sign in with Keycloak' }).click()
  await expect(page).toHaveURL(new RegExp(`^${escape(ISSUER)}/protocol/openid-connect/auth`))
  await page.locator('#username').fill(username)
  await page.locator('#password').fill(password)
  await page.locator('#kc-login').click()
}
const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** An administrator's access token for Keycloak's admin API. */
async function keycloakAdmin(): Promise<string> {
  const res = await fetch(`${KEYCLOAK}/realms/master/protocol/openid-connect/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'password',
      client_id: 'admin-cli',
      username: 'admin',
      password: 'admin',
    }),
  })
  expect(res.status, 'admin login').toBe(200)
  return ((await res.json()) as { access_token: string }).access_token
}
async function adminCall(path: string, init: RequestInit = {}) {
  const token = await keycloakAdmin()
  return fetch(`${KEYCLOAK}/admin/realms/${REALM}${path}`, {
    ...init,
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
  })
}

test('the round trip with a real provider: login, groups become roles, the server accepts the token', async ({
  browser,
}) => {
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  const page = await ctx.newPage()
  await page.goto('/repos/acme/webapp')
  await expect(page.getByRole('heading', { name: 'Sign in to AgentGit' })).toBeVisible()
  await signInAtKeycloak(page, 'alice', 'alice-password')
  // Back in the app, where the person was going, as the person Keycloak said they are.
  await expect(page).toHaveURL(`${server.baseURL}/repos/acme/webapp`)
  await expect(page.getByTestId('user-menu')).toContainText('user:alice')
  // The `staff` group became a reader of acme/*: the repository is visible, administration is not offered.
  await expect(page.getByTestId('board-summary')).toBeVisible()
  await page.getByTestId('user-menu').click()
  await page.getByRole('menuitem', { name: 'Credentials and roles' }).click()
  await expect(page.getByText(/reader/).first()).toBeVisible()
  await ctx.close()
})

test('a person in the platform group is an administrator', async ({ browser }) => {
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  const page = await ctx.newPage()
  await page.goto('/signin')
  await signInAtKeycloak(page, 'dana', 'dana-password')
  await expect(page.getByTestId('user-menu')).toContainText('user:dana')
  await page.goto('/repos/acme/webapp/admin')
  await expect(page.getByRole('heading', { name: 'Administration', level: 1 })).toBeVisible()
  await ctx.close()
})

test('a person without a group is signed in but sees nothing', async ({ browser }) => {
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  const page = await ctx.newPage()
  await page.goto('/signin')
  await signInAtKeycloak(page, 'nobody', 'nobody-password')
  await expect(page.getByTestId('user-menu')).toContainText('user:nobody')
  await page.goto('/')
  await expect(page.getByText('No repositories yet')).toBeVisible()
  await ctx.close()
})

test('a wrong password stays at the provider, which says so', async ({ browser }) => {
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  const page = await ctx.newPage()
  await page.goto('/signin')
  await page.getByRole('button', { name: 'Sign in with Keycloak' }).click()
  await page.locator('#username').fill('alice')
  await page.locator('#password').fill('not-the-password')
  await page.locator('#kc-login').click()
  await expect(page.getByText(/Invalid username or password/i)).toBeVisible()
  await expect(page).toHaveURL(new RegExp(`^${escape(ISSUER)}/`))
  await ctx.close()
})

test('a reload re-authenticates silently with the provider’s own session, and signing out ends that session too', async ({
  browser,
}) => {
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  const page = await ctx.newPage()
  await page.goto('/signin')
  await signInAtKeycloak(page, 'alice', 'alice-password')
  await expect(page.getByTestId('user-menu')).toContainText('user:alice')

  // No token is stored anywhere; a reload asks Keycloak again (prompt=none) and its cookie answers.
  await page.goto('/repos/acme/webapp/streams')
  await expect(page.getByTestId('user-menu')).toContainText('user:alice')
  await expect(page).toHaveURL(`${server.baseURL}/repos/acme/webapp/streams`)

  await page.getByTestId('user-menu').click()
  await page.getByRole('menuitem', { name: 'Sign out' }).click()
  await expect(page).toHaveURL(`${server.baseURL}/signin`)
  // Keycloak's session is gone: pressing the button now shows its login form instead of signing in at once.
  await page.getByRole('button', { name: 'Sign in with Keycloak' }).click()
  await expect(page.locator('#username')).toBeVisible()
  await ctx.close()
})

test('the session ends with an explanation when the provider revokes it and the token runs out', async ({
  browser,
}) => {
  test.setTimeout(120_000)
  // A short-lived token: the UI renews it silently with the refresh token; once Keycloak’s session is gone the
  // renewal is refused, the person is warned, and the session ends when the token does.
  const clients = await (await adminCall(`/clients?clientId=${CLIENT}`)).json()
  const id = (clients as { id: string; attributes: Record<string, string> }[])[0]!.id
  const full = await (await adminCall(`/clients/${id}`)).json()
  const original = full.attributes['access.token.lifespan']
  const set = async (lifespan: string | undefined) => {
    const attributes = { ...full.attributes }
    if (lifespan === undefined) delete attributes['access.token.lifespan']
    else attributes['access.token.lifespan'] = lifespan
    const res = await adminCall(`/clients/${id}`, {
      method: 'PUT',
      body: JSON.stringify({ ...full, attributes }),
    })
    expect(res.status, 'update the client').toBe(204)
  }
  await set('12')
  const ctx = await browser.newContext({ baseURL: server.baseURL })
  try {
    const page = await ctx.newPage()
    await page.goto('/signin')
    await signInAtKeycloak(page, 'alice', 'alice-password')
    await expect(page.getByTestId('user-menu')).toContainText('user:alice')

    // Keycloak ends every session of the person.
    const users = await (await adminCall('/users?username=alice&exact=true')).json()
    const logout = await adminCall(`/users/${(users as { id: string }[])[0]!.id}/logout`, {
      method: 'POST',
    })
    expect(logout.status).toBe(204)

    await expect(page.getByRole('heading', { name: 'Your session ended', level: 1 })).toBeVisible({
      timeout: 60_000,
    })
    await expect(page).toHaveURL(/\/unauthorized/)
    await expect(page.getByTestId('user-menu')).toHaveCount(0)
  } finally {
    await set(original)
    await ctx.close()
  }
})
