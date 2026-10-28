import { resolve } from 'node:path'
import { test, expect } from './fixtures.ts'
import { defaultUi, startServer } from './support/server.ts'
import { suffix } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

test('creates tasks, adds a prerequisite and finishes dependent work through permanent task pages', async ({
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
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/tasks-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Agents & work', exact: true }).click()
    await page.getByRole('tab', { name: 'Tasks', exact: true }).click()
    for (const title of ['Build foundation', 'Ship feature']) {
      await page.getByRole('button', { name: 'New task', exact: true }).click()

      const dialog = page.getByRole('dialog', { name: 'New task', exact: true })
      await dialog.getByRole('textbox', { name: 'Title', exact: true }).fill(title)
      await dialog.getByRole('button', { name: 'Create task', exact: true }).click()
      await expect(dialog).not.toBeVisible()
    }
    await page.getByRole('link', { name: 'Ship feature', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Ship feature', exact: true })).toBeVisible()
    const featureUrl = page.url()
    await page
      .getByLabel('Add prerequisite', { exact: true })
      .selectOption({ label: 'Build foundation · open' })
    await page.getByRole('button', { name: 'Add prerequisite', exact: true }).click()
    await expect(
      page.getByText('Waiting for unfinished prerequisites.', { exact: true }),
    ).toBeVisible()
    await page
      .getByRole('region', { name: 'Task prerequisites', exact: true })
      .getByRole('link', { name: 'Build foundation' })
      .click()
    await expect(
      page
        .getByRole('region', { name: 'Dependent tasks', exact: true })
        .getByRole('link', { name: 'Ship feature' }),
    ).toBeVisible()
    await expectAccessible(page, info, 'task detail')
    await page.screenshot({ path: info.outputPath('task-detail.png'), fullPage: true })
    await page.getByRole('button', { name: 'Mark done…', exact: true }).click()
    await page
      .getByRole('dialog', { name: 'Mark this task done?' })
      .getByRole('button', { name: 'Confirm completion', exact: true })
      .click()
    await expect(page.getByText('Task marked done.', { exact: true })).toBeVisible()
    await page.goto(featureUrl)
    await expect(page.getByText('All prerequisites are complete.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Cancel task…', exact: true }).click()
    const cancel = page.getByRole('dialog', { name: 'Cancel this task?', exact: true })
    await cancel.getByRole('button', { name: 'Keep task unchanged', exact: true }).click()
    await expect(page.getByText('All prerequisites are complete.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Cancel task…', exact: true }).click()
    await cancel.getByRole('button', { name: 'Confirm cancellation', exact: true }).click()
    await expect(page.getByText('Task cancelled.', { exact: true })).toBeVisible()
    await page.reload()
    await expect(page.getByRole('heading', { name: 'Ship feature', exact: true })).toBeVisible()
    await expect(page.locator('.state').getByText('cancelled', { exact: true })).toBeVisible()
    await page.screenshot({ path: info.outputPath('task-detail-finished.png'), fullPage: true })
    await page.getByRole('link', { name: 'Back to tasks', exact: true }).click()
    await expect(page).toHaveURL(/\/agents\?tab=tasks$/)
    await page.reload()
    await expect(page.getByRole('tab', { name: 'Tasks', exact: true })).toHaveAttribute(
      'aria-selected',
      'true',
    )
    await expect(page.getByRole('link', { name: 'Ship feature', exact: true })).toBeVisible()
  } finally {
    await page.close()
    await server.stop()
  }
})
