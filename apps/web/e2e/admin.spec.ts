import { expect, test } from './fixtures.ts'
import { Api } from './support/http.ts'
import { freshRepo, suffix } from './support/world.ts'

test.describe('administration', () => {
  test('mints a token that is shown exactly once, then never again', async ({
    page,
    cast,
    baseURL,
  }) => {
    await page.goto('/repos/acme/webapp/admin')
    await page.getByRole('tab', { name: 'Credentials' }).click()
    await page.getByTestId('mint-open').click()
    const dialog = page.getByRole('dialog', { name: 'Mint a token' })
    await expect(dialog.getByLabel('Description')).toBeFocused()
    await dialog.getByLabel('Identity').fill('agent:doc-bot')
    const description = `docs bot ${suffix()}`
    await dialog.getByLabel('Description').fill(description)
    await dialog.getByLabel('Repositories').fill('acme/webapp')
    await dialog.getByRole('button', { name: 'Mint token' }).click()

    // The reveal: masked by default, copyable, with a clear warning.
    const reveal = page.getByRole('dialog', { name: 'Your new token' })
    await expect(reveal).toContainText('This is the only time you will see this token')
    const field = reveal.getByTestId('minted-token')
    const token = await field.inputValue()
    expect(token).toMatch(/^agt_[0-9a-f]+_[\w-]{20,}$/)
    // Shown in full so it can be checked and copied, selected and focused; it can be hidden with one click.
    await expect(field).toBeFocused()
    await expect(field).not.toHaveClass(/masked/)
    await reveal.getByRole('button', { name: 'Hide token' }).click()
    await expect(field).toHaveClass(/masked/)
    await reveal.getByRole('button', { name: 'Show token' }).click()
    await expect(field).not.toHaveClass(/masked/)
    await expect(reveal).toContainText('doc-bot')
    await expect(reveal.getByRole('button', { name: 'Copy token' })).toBeVisible()

    // The token is real: the server accepts it, as the identity and with the scope that was asked for.
    const who = await new Api(baseURL!, token).get('/v1/whoami')
    expect(who.status).toBe(200)
    expect(who.json.identity).toBe('agent:doc-bot')
    expect(who.json.scope.repos).toEqual(['acme/webapp'])

    await reveal.getByTestId('minted-done').click()
    await expect(reveal).toHaveCount(0)

    // From here on the secret is gone from the page: only the credential's id and description remain.
    const row = page.getByRole('row').filter({ hasText: description })
    await expect(row).toBeVisible()
    await expect(row).toContainText('doc-bot')
    const html = await page.content()
    expect(html).not.toContain(token)
    await page.reload()
    await page.getByRole('tab', { name: 'Credentials' }).click()
    // The list starts with your own credentials; look the agent's up by identity.
    await page.getByLabel('Subject').fill('agent:doc-bot')
    await expect(page.getByRole('row').filter({ hasText: description })).toBeVisible()
    expect(await page.content()).not.toContain(token)
    expect(
      await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage })),
    ).not.toContain(token)

    // Revoking it from the list takes effect at once.
    await row.getByRole('button', { name: 'Revoke' }).click()
    const confirm = page.getByRole('alertdialog', { name: 'Revoke this credential?' })
    await confirm.getByRole('button', { name: 'Revoke', exact: true }).click()
    await expect(page.getByRole('row').filter({ hasText: description })).toHaveCount(0)
    expect((await new Api(baseURL!, token).get('/v1/whoami')).status).toBe(401)
    await page.getByLabel('Include revoked').check()
    await expect(page.getByRole('row').filter({ hasText: description })).toContainText('revoked')
    void cast
  })

  test('the policy editor explains mistakes, the server has the last word, and saving makes a new version', async ({
    page,
    cast,
    allowConsole,
  }) => {
    allowConsole(/status of 400/)
    const { repo } = await freshRepo(cast, 'policy')
    await page.goto(`/repos/${repo}/admin`)
    const editor = page.getByTestId('policy-editor')
    await expect(editor).toContainText('Version 0 is in force (the built-in default)')
    await page.getByTestId('policy-advanced').locator('summary').click()
    const text = page.getByTestId('policy-text')
    await expect(text).toHaveValue(/approvals/) // a starter document to edit

    // A typo is caught while typing, with the line it is on.
    await text.fill('{\n  "approvals": { "mim": 1 }\n}')
    const issues = page.getByTestId('policy-issues')
    await expect(issues).toContainText('line 2')
    await expect(issues).toContainText(/mim/)
    await expect(page.getByTestId('policy-save')).toBeEnabled()

    // The server is strict and says why; nothing is stored.
    await page.getByTestId('policy-save').click()
    await expect(page.getByTestId('policy-server-error')).toContainText(
      'The server did not accept this policy',
    )
    const still = await cast.as('dana').get(`/v1/repos/${repo}/policy?stream=main`)
    expect(still.json.version).toBe(0)

    // Fix it and save: version 1 exists, and a policy change is announced.
    await text.fill('{\n  "approvals": { "min": 1 }\n}')
    await expect(page.getByTestId('policy-server-error')).toBeVisible()
    await page.getByTestId('policy-save').click()
    await expect(page.getByText('Policy version 1 saved')).toBeVisible()
    await expect(editor).toContainText('Version 1 is in force')
    await expect(page.getByTestId('policy-server-error')).toHaveCount(0)

    // A second version, and a readable diff between the two.
    await text.fill('{\n  "approvals": { "min": 2, "forbid_self_approval": true }\n}')
    await page.getByTestId('policy-save').click()
    await expect(editor).toContainText('Version 2 is in force')
    const diff = page.getByTestId('policy-diff')
    await expect(diff).toContainText('"min": 2')
    await expect(diff).toContainText('removed:')
    await expect(diff).toContainText('added:')
    const stored = await cast.as('dana').get(`/v1/repos/${repo}/policy/history?stream=main`)
    expect(stored.json.versions.map((v: { version: number }) => v.version)).toEqual([1, 2])
  })

  test('the demo policy has two versions and the diff shows what the second one added', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await expect(page.getByTestId('policy-editor')).toContainText('Version 2 is in force')
    await expect(page.getByTestId('policy-text')).toHaveValue(/protected_paths/)
    const diff = page.getByTestId('policy-diff')
    await expect(diff).toContainText('protected_paths')
    await expect(diff).toContainText('.github/**')
  })

  test('roles: grant, see, and revoke after a confirmation', async ({ page, cast }) => {
    const { repo } = await freshRepo(cast, 'roles')
    await page.goto(`/repos/${repo}/admin`)
    await page.getByRole('tab', { name: 'Roles' }).click()
    await page.getByTestId('grant-open').click()
    const dialog = page.getByRole('dialog', { name: 'Grant a role' })
    await dialog.getByLabel('Who').fill('user:zed')
    await dialog.getByLabel('Role').selectOption('reviewer')
    await dialog.getByRole('button', { name: 'Grant', exact: true }).click()
    const row = page.getByRole('row').filter({ hasText: 'zed' })
    await expect(row).toContainText('reviewer')
    const bound = await cast.as('dana').get(`/v1/roles?subject=user:zed`)
    expect(bound.json.bindings).toHaveLength(1)

    await row.getByRole('button', { name: 'Revoke role' }).click()
    const confirm = page.getByRole('alertdialog', { name: 'Revoke this role?' })
    await confirm.getByRole('button', { name: 'Revoke', exact: true }).click()
    await expect(page.getByRole('row').filter({ hasText: 'zed' })).toHaveCount(0)
  })

  test('settings and quota are saved by the server', async ({ page, cast }) => {
    const { repo } = await freshRepo(cast, 'settings')
    await page.goto(`/repos/${repo}/admin`)
    await page.getByRole('tab', { name: 'Settings and quota' }).click()
    await page.getByLabel('Description').fill('Quota test repository')
    await page.getByRole('button', { name: 'Save settings' }).click()
    await expect(page.getByText(/saved/i).first()).toBeVisible()
    const got = await cast.as('dana').get(`/v1/repos/${repo}`)
    expect(got.json.description).toBe('Quota test repository')
  })

  test('garbage collection starts as a dry run; the real thing stays out of reach until then', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'gc')
    await page.goto(`/repos/${repo}/admin`)
    await page.getByRole('tab', { name: 'GC and fsck' }).click()
    const real = page.getByTestId('gc-real')
    await expect(real).toHaveAttribute('aria-disabled', 'true')
    await expect(real).toHaveAccessibleDescription(/Run a dry run first/)
    await page.getByTestId('gc-dry').click()
    await expect(page.getByTestId('gc-result')).toContainText('Dry run: nothing was deleted')
    // A fresh repository has nothing to collect, and the page says so instead of offering a deletion.
    await expect(real).toHaveAccessibleDescription(/Nothing to delete/)
  })

  test('fsck re-reads every object and reports a healthy repository', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await page.getByRole('tab', { name: 'GC and fsck' }).click()
    await page.getByTestId('fsck-run').click()
    const result = page.getByTestId('fsck-result')
    await expect(result).toContainText('Healthy')
    await expect(result).toContainText(/\d+ objects checked/)
  })

  test('the audit log lists who did what, can be filtered, and its hash chain can be verified', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await page.getByRole('tab', { name: 'Audit log' }).click()
    const table = page.getByRole('table').last()
    await expect(table).toContainText('proposal.create')
    await expect(table).toContainText('alice')
    // The filter matches the actor, the target and the details.
    await page.getByLabel('Contains').fill('carol')
    await page.getByRole('button', { name: 'Filter' }).click()
    await expect(table).toContainText('queue.enqueue')
    await expect(table).toContainText('stream.integrate')
    await expect(table).not.toContainText('proposal.create')

    await page.getByTestId('audit-verify').click()
    const result = page.getByTestId('audit-verify-result')
    await expect(result).toContainText('Chain intact')
    await expect(result).toContainText(/\d+ entries/)
  })

  test('pins keep revisions alive: the release pin is listed, and one can be added and removed', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await page.getByRole('tab', { name: 'Pins' }).click()
    await expect(page.getByRole('row').filter({ hasText: state.demo.pin })).toContainText(
      'Release 2.3.0 (sessions + billing)',
    )

    const name = `keep-${suffix()}`
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(name)
    await page.getByRole('textbox', { name: 'Revision' }).fill('main~1')
    await page.getByRole('textbox', { name: 'Reason' }).fill('Rollback target')
    await page.getByRole('button', { name: 'Pin', exact: true }).click()
    const row = page.getByRole('row').filter({ hasText: name })
    await expect(row).toContainText('Rollback target')
    await row.getByRole('button', { name: 'Remove pin' }).click()
    await expect(page.getByRole('row').filter({ hasText: name })).toHaveCount(0)
  })
})

test.describe('credentials and roles for yourself (outside any repository)', () => {
  test.use({ persona: 'alice' })

  test('anyone can mint a token for themselves, and only for themselves', async ({
    page,
    baseURL,
  }) => {
    await page.goto('/access')
    await expect(page.getByRole('heading', { name: /Credentials/ }).first()).toBeVisible()
    await page.getByTestId('mint-open').click()
    const dialog = page.getByRole('dialog', { name: 'Mint a token' })
    await expect(dialog.getByLabel('Identity')).toBeDisabled()
    await dialog.getByLabel('Description').fill('my laptop')
    await dialog.getByRole('button', { name: 'Mint token' }).click()
    const token = await page.getByTestId('minted-token').inputValue()
    expect((await new Api(baseURL!, token).get('/v1/whoami')).json.identity).toBe('user:alice')
    await page.getByTestId('minted-done').click()
  })
})
