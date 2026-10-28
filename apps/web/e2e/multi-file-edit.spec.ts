import { resolve } from 'node:path'
import type { Page } from '@playwright/test'
import { test, expect } from './fixtures.ts'
import { defaultUi, startServer } from './support/server.ts'
import { suffix } from './support/world.ts'

async function land(page: Page) {
  await page.getByTestId('act-integrate').click()
  const dialog = page.getByRole('dialog', { name: 'Land this change request' })
  await dialog.getByRole('button', { name: 'Land', exact: true }).click()
  await expect(dialog.getByText('Landed', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
}
async function publish(page: Page, title: string) {
  await page.getByRole('button', { name: 'Review changes', exact: true }).click()
  await page.getByLabel('Change request title').fill(title)
  await page.getByRole('button', { name: 'Open change request', exact: true }).click()
  await expect(page.getByTestId('proposal-title')).toHaveText(title)
}
async function openFile(page: Page, path: string) {
  await page.getByRole('button', { name: 'Open existing file' }).click()
  const finder = page.getByRole('dialog', { name: 'Find a file' })
  await finder.getByLabel('File name or path').fill(path)
  await finder.getByRole('link', { name: path, exact: true }).click()
  await expect(page.getByLabel('File path', { exact: true })).toHaveValue(path)
}

test('creates, recovers and edits several files in one change request through the UI', async ({
  openAs,
}, info) => {
  const server = await startServer({ ui: defaultUi(resolve(import.meta.dirname, '../dist')) })
  const page = await openAs(null)
  page.on('dialog', (dialog) => dialog.accept())
  try {
    await page.goto(`${server.baseURL}/signin`)
    await page.getByLabel('Access token').fill(server.adminToken)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await page.getByRole('button', { name: 'Continue', exact: true }).click()
    await page.getByTestId('home-new-repo').click()
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/multi-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByRole('link', { name: 'Edit multiple files', exact: true }).click()
    for (const name of ['one', 'two']) {
      await page.getByLabel('New file path').fill(`docs/${name}.txt`)
      await page.getByRole('button', { name: 'Add file', exact: true }).click()
      await page.getByLabel('Contents', { exact: true }).fill(`${name} original\n`)
    }
    const downloaded = page.waitForEvent('download')
    await page.getByRole('button', { name: 'Download draft', exact: true }).click()
    const savedPath = info.outputPath('portable-draft.json')
    await (await downloaded).saveAs(savedPath)
    await page.getByRole('button', { name: 'Start over', exact: true }).click()
    await page.getByRole('button', { name: 'Discard draft', exact: true }).click()
    await page.getByLabel('Saved change draft').setInputFiles(savedPath)
    await expect(
      page.getByText('Draft recovered with its original starting revision.'),
    ).toBeVisible()
    await page.reload()
    await expect(
      page.getByText('Draft recovered with its original starting revision.'),
    ).toBeVisible()
    await page
      .getByRole('complementary', { name: 'Files in this change' })
      .getByRole('button')
      .filter({ hasText: 'docs/two.txt' })
      .click()
    await expect(page.getByLabel('Contents', { exact: true })).toHaveValue('two original\n')
    await page.screenshot({ path: info.outputPath('multi-file-draft.png'), fullPage: true })
    await publish(page, 'Add both documents')
    await land(page)
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByRole('link', { name: 'Edit multiple files', exact: true }).click()
    await openFile(page, 'docs/one.txt')
    await page.getByLabel('File path', { exact: true }).fill('docs/renamed.txt')
    await page.getByLabel('Contents', { exact: true }).fill('Edited and renamed together\n')
    await openFile(page, 'docs/two.txt')
    await page.getByRole('button', { name: 'Delete file', exact: true }).click()
    await page.getByRole('button', { name: 'Review changes', exact: true }).click()
    await expect(page.getByRole('region', { name: 'Review changes' })).toContainText('Renamed')
    await expect(page.getByRole('region', { name: 'Review changes' })).toContainText('Deleted')
    await page.screenshot({ path: info.outputPath('multi-file-review.png'), fullPage: true })
    await page.getByLabel('Change request title').fill('Rename and remove together')
    await page.getByRole('button', { name: 'Open change request', exact: true }).click()
    await expect(page.getByTestId('proposal-title')).toHaveText('Rename and remove together')
    await land(page)
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByRole('link', { name: 'docs', exact: true }).click()
    await expect(page.getByRole('link', { name: 'one.txt', exact: true })).toHaveCount(0)
    await expect(page.getByRole('link', { name: 'two.txt', exact: true })).toHaveCount(0)
    await page.getByRole('link', { name: 'renamed.txt', exact: true }).click()
    await expect(page.getByTestId('file-viewer')).toContainText('Edited and renamed together')
    await page.reload()
    await expect(page.getByTestId('file-viewer')).toContainText('Edited and renamed together')
  } finally {
    await page.close()
    await server.stop()
  }
})
