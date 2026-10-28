import { resolve } from 'node:path'
import { test, expect } from './fixtures.ts'
import { defaultUi, startServer } from './support/server.ts'
import { suffix } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

test('a browser-created change can be reviewed file by file and resumed', async ({
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
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/review-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByRole('button', { name: 'Upload files', exact: true }).click()
    await page.getByLabel('Files to upload').setInputFiles([
      { name: 'one.ts', mimeType: 'text/plain', buffer: Buffer.from('export const one = 1\n') },
      { name: 'two.md', mimeType: 'text/markdown', buffer: Buffer.from('# Second file\n') },
    ])
    await page.getByRole('button', { name: 'Review upload' }).click()
    await page.getByRole('button', { name: 'Open change request', exact: true }).click()
    await expect(page.getByTestId('proposal-title')).toHaveText('Upload 2 files')
    await page.getByLabel('Filter changed files').fill('TWO.')
    await expect(page.getByRole('region', { name: 'Changes to two.md', exact: true })).toBeVisible()
    await expect(page.getByRole('region', { name: 'Changes to one.ts', exact: true })).toHaveCount(
      0,
    )
    await page.getByLabel('Viewed two.md', { exact: true }).check()
    await page.getByLabel('Hide viewed files', { exact: true }).check()
    await expect(page.getByText('No matching files', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Show all files', exact: true }).click()
    await expect(page.getByRole('region', { name: 'Changes to one.ts', exact: true })).toBeVisible()
    await page.reload()
    await expect(page.getByLabel('Viewed two.md', { exact: true })).toBeChecked()
    await expect(page.getByLabel('Viewed one.ts', { exact: true })).not.toBeChecked()
    await expect(page.getByText('1 of 2 loaded files viewed', { exact: true })).toBeVisible()
    await expectAccessible(page, info, 'file review workbench')
    const size = page.viewportSize()!
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      size.width,
    )
    await page.screenshot({ path: info.outputPath('review-workbench.png'), fullPage: true })
    await page.getByTestId('act-review').click()
    const dialog = page.getByRole('dialog', { name: 'Write a review' })
    await dialog.getByRole('radio', { name: 'Comment', exact: true }).check()
    const comment = 'Checked both files, including the second document.'
    await dialog.getByRole('textbox', { name: 'Comment', exact: true }).fill(comment)
    await dialog.getByRole('button', { name: 'Keep draft and close', exact: true }).click()
    await page.reload()
    await page.getByTestId('act-review').click()
    await expect(dialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(comment)
    await expect(dialog.getByRole('radio', { name: 'Comment', exact: true })).toBeChecked()
    await dialog.getByRole('button', { name: 'Submit review', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    await page.getByRole('tab', { name: /Reviews/ }).click()
    await expect(page.getByTestId('reviews-tab')).toContainText(comment)
    await page.reload()
    await page.getByTestId('act-review').click()
    await expect(dialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue('')
  } finally {
    await page.close()
    await server.stop()
  }
})
