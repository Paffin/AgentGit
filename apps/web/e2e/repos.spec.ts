import { expect, test } from './fixtures.ts'
import { EMPTY_REPO, HOSTILE_REPO } from './support/seed.ts'
import { suffix } from './support/world.ts'

test.describe('repositories', () => {
  test('lists what you may read, with the real counts filled in', async ({ page }) => {
    await page.goto('/')
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
    await page.goto('/')
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
    await page.goto('/')
    await page.getByRole('link', { name: 'acme/webapp' }).click()
    await expect(page).toHaveURL(/\/repos\/acme\/webapp$/)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByRole('link', { name: 'Live board' })).toBeVisible()
  })

  test('creates a repository from the dialog and lands on its (empty) board', async ({
    page,
    cast,
    allowConsole,
  }) => {
    allowConsole(/status of 400/)
    const name = `acme/made-in-ui-${suffix()}`
    await page.goto('/')
    await page.getByRole('button', { name: 'New repository' }).click()
    const dialog = page.getByRole('dialog', { name: 'New repository' })
    await expect(dialog.getByLabel('Name')).toBeFocused()

    // The server's own validation is shown, not hidden.
    await dialog.getByLabel('Name').fill('Not A Valid Name')
    await dialog.getByRole('button', { name: 'Create repository' }).click()
    await expect(dialog.getByRole('alert')).toContainText('Could not create the repository')

    await dialog.getByLabel('Name').fill(name)
    await dialog.getByLabel('Description').fill('Created from the web UI by an end-to-end test')
    await dialog.getByRole('button', { name: 'Create repository' }).click()
    await expect(dialog).toHaveCount(0)

    const created = await cast.as('dana').get(`/v1/repos/${name}`)
    expect(created.status).toBe(200)
    expect(created.json.description).toBe('Created from the web UI by an end-to-end test')
    await expect(page).toHaveURL(new RegExp(`/repos/${name}$`))
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByTestId('proposals-panel')).toContainText('No open proposals')
  })

  test('a duplicate name is reported by the server and nothing changes', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 409/)
    await page.goto('/')
    await page.getByRole('button', { name: 'New repository' }).click()
    const dialog = page.getByRole('dialog', { name: 'New repository' })
    await dialog.getByLabel('Name').fill('acme/webapp')
    await dialog.getByRole('button', { name: 'Create repository' }).click()
    await expect(dialog.getByRole('alert')).toContainText(/already exists|exists/i)
    await expect(dialog).toBeVisible()
  })
})

test.describe('repositories for someone who may not create them', () => {
  test.use({ persona: 'erin' })

  test('the create button is disabled and explains why', async ({ page }) => {
    await page.goto('/')
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
    await page.goto('/')
    await expect(page.getByText('No repositories yet')).toBeVisible()
    await expect(page.getByText(/once an administrator grants you a role/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'New repository' })).toBeDisabled()
  })
})
