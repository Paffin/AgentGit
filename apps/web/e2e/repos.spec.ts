import { expect, test } from './fixtures.ts'
import { EMPTY_REPO, HOSTILE_REPO } from './support/seed.ts'
import { suffix } from './support/world.ts'

test.describe('repositories', () => {
  test('lists what you may read, with the real counts filled in', async ({ page }) => {
    await page.goto('/repos')
    const table = page.getByRole('table', { name: 'Repositories' })
    await expect(table.getByRole('link', { name: 'acme/webapp' })).toBeVisible()
    await expect(table.getByRole('link', { name: EMPTY_REPO })).toBeVisible()
    await expect(table.getByRole('link', { name: HOSTILE_REPO })).toBeVisible()

    // The list endpoint reports zero counts; the page asks each repository for the real ones.
    const row = table.getByRole('row').filter({ hasText: 'acme/webapp' })
    await expect(row.getByRole('cell').nth(3)).not.toHaveText('0')
    await expect(row.getByRole('cell').nth(3)).not.toHaveText('–')
    const empty = table.getByRole('row').filter({ hasText: EMPTY_REPO })
    await expect(empty.getByRole('cell').nth(3)).toHaveText('0')
  })

  test('searches by name and by description, and says so when nothing matches', async ({
    page,
  }) => {
    await page.goto('/repos')
    const table = page.getByRole('table', { name: 'Repositories' })
    await expect(table.getByRole('link', { name: 'acme/webapp' })).toBeVisible()

    await page.getByRole('searchbox', { name: 'Search repositories' }).fill('mobile client')
    await expect(table.getByRole('link', { name: EMPTY_REPO })).toBeVisible()
    await expect(table.getByRole('link', { name: 'acme/webapp' })).toHaveCount(0)
    await expect(page.getByText('1 repository')).toBeVisible()

    await page.getByRole('searchbox', { name: 'Search repositories' }).fill('zzz-nothing')
    await expect(page.getByText('No repository matches')).toBeVisible()
  })

  test('opens a repository on its live board', async ({ page }) => {
    await page.goto('/repos')
    await page.getByRole('link', { name: 'acme/webapp' }).click()
    await expect(page).toHaveURL(/\/repos\/acme\/webapp$/)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByRole('link', { name: 'Overview' })).toBeVisible()
  })

  test('creates a repository with the wizard and lands on its (empty) overview', async ({
    page,
    cast,
  }) => {
    const name = `acme/made-in-ui-${suffix()}`
    await page.goto('/repos')
    await page.getByRole('link', { name: 'New repository' }).click()
    await expect(page.getByRole('heading', { name: 'Start a repository', level: 1 })).toBeVisible()
    await expect(page.getByRole('textbox', { name: 'Name', exact: true })).toBeFocused()

    // A name that cannot work is explained before anything is sent.
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Not A Valid Name')
    await expect(page.getByText('Write it as owner/name')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Next: choose the rules' })).toBeDisabled()

    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(name)
    await page
      .getByLabel('What lives here (optional)')
      .fill('Created from the web UI by an end-to-end test')
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-agents').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await expect(page.getByText('Agents may land a change by themselves')).toBeVisible()
    await page.getByTestId('create-repo').click()

    await expect(page).toHaveURL(new RegExp(`/repos/${name}\\?welcome=1$`))
    const created = await cast.as('dana').get(`/v1/repos/${name}`)
    expect(created.status).toBe(200)
    expect(created.json.description).toBe('Created from the web UI by an end-to-end test')
    // The chosen rules were stored: version 1, with agent autonomy on.
    const policy = await cast.as('dana').get(`/v1/repos/${name}/policy?stream=main`)
    expect(policy.json.version).toBe(1)
    expect(JSON.parse(policy.json.document).agent_autonomy.auto_integrate).toBe(true)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByTestId('proposals-panel')).toContainText('No open change requests')
    await expect(page.getByTestId('getting-started')).toContainText('2 of 5 done')
  })

  test('a duplicate name is reported by the server and nothing changes', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 409/)
    await page.goto('/repos/new')
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('acme/webapp')
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await expect(page.getByRole('alert').filter({ hasText: 'Could not create' })).toContainText(
      /already exists|exists/i,
    )
    // Back on the first step, where the name can be fixed.
    await expect(page.getByRole('textbox', { name: 'Name', exact: true })).toBeVisible()
  })
})

test.describe('repositories for someone who may not create them', () => {
  test.use({ persona: 'erin' })

  test('the create button is disabled and explains why', async ({ page }) => {
    await page.goto('/repos')
    const button = page.getByRole('button', { name: 'New repository' })
    await expect(button).toBeDisabled()
    // Kept focusable, with the reason as its description (and as a hover title).
    await expect(button).toHaveAccessibleDescription(/requires the admin role/)
    await expect(button).toHaveAttribute('title', /requires the admin role/)
  })
})

test.describe('repositories for someone with no roles', () => {
  test.use({ persona: null })

  test('shows an honest empty state, not an error', async ({ page, cast }) => {
    const admin = cast.as('admin')
    const minted = await admin.ok_(
      admin.post('/v1/credentials', { subject: 'user:nobody', ttl_secs: 600 }),
      'mint',
    )
    await page.addInitScript((t) => sessionStorage.setItem('agentgit.token', t), minted.token)
    await page.goto('/repos')
    await expect(page.getByText('No repositories yet')).toBeVisible()
    await expect(page.getByText(/once an administrator grants you a role/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'New repository' })).toBeDisabled()
  })
})
