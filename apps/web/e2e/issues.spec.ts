import { resolve } from 'node:path'
import { test, expect } from './fixtures.ts'
import { defaultUi, startServer } from './support/server.ts'
import { suffix } from './support/world.ts'

test('create, recover, discuss and reopen an issue through the browser', async ({
  openAs,
}, info) => {
  const server = await startServer({ ui: defaultUi(resolve(import.meta.dirname, '../dist')) })
  const page = await openAs(null)
  try {
    await page.goto(`${server.baseURL}/signin`)
    await page.getByLabel('Access token').fill(server.adminToken)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue', exact: true }).click()
    await page.getByTestId('home-new-repo').click()
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/issues-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Issues', exact: true }).click()
    await page.getByRole('button', { name: 'New issue', exact: true }).click()
    await page.getByRole('textbox', { name: 'Title', exact: true }).fill('Move the team project')
    await page
      .getByLabel('Description', { exact: true })
      .fill('Keep the team discussion and history.')
    await page.reload()
    await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue('Move the team project')
    await page.getByRole('button', { name: 'Create issue', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Move the team project #1' })).toBeVisible()
    const issueUrl = page.url()
    await page.getByLabel('Add a comment').fill('Ready for the migration review.')
    await page.getByRole('button', { name: 'Comment', exact: true }).click()
    await expect(page.getByText('Ready for the migration review.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Close issue', exact: true }).click()
    await page
      .getByRole('dialog', { name: 'Close this issue?' })
      .getByRole('button', { name: 'Close issue', exact: true })
      .click()
    await expect(page.getByRole('button', { name: 'Reopen issue', exact: true })).toBeVisible()
    await page.reload()
    await expect(page.getByText('Ready for the migration review.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Reopen issue', exact: true }).click()
    await page
      .getByRole('dialog', { name: 'Reopen this issue?' })
      .getByRole('button', { name: 'Reopen issue', exact: true })
      .click()
    await expect(page.getByRole('button', { name: 'Close issue', exact: true })).toBeVisible()
    await page.screenshot({ path: info.outputPath('issue-discussion.png'), fullPage: true })
    await page.getByRole('link', { name: 'Issues', exact: true }).click()
    await page.getByLabel('Search issues').fill('Move the team')
    await page.getByRole('button', { name: 'Apply', exact: true }).click()
    await expect(
      page.getByRole('link', { name: 'Move the team project', exact: true }),
    ).toHaveAttribute('href', new URL(issueUrl).pathname)
  } finally {
    await page.close()
    await server.stop()
  }
})
