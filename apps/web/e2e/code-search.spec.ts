import { resolve } from 'node:path'
import { test, expect } from './fixtures.ts'
import { defaultUi, startServer } from './support/server.ts'
import { suffix } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

test('searches uploaded contents, continues on the same revision and opens the matching line', async ({
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
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/search-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByRole('button', { name: 'Upload files', exact: true }).click()
    await page.getByLabel('Files to upload').setInputFiles([
      {
        name: 'README.md',
        mimeType: 'text/markdown',
        buffer: Buffer.from('# Readme\nneedle here\n'),
      },
      {
        name: 'src.txt',
        mimeType: 'text/plain',
        buffer: Buffer.from(
          Array.from({ length: 2000 }, (_, i) =>
            i < 1940 ? `filler ${i}` : i === 1940 ? 'Needle mixed' : `needle ${i}`,
          ).join('\n'),
        ),
      },
      {
        name: 'asset.bin',
        mimeType: 'application/octet-stream',
        buffer: Buffer.from('needle\0binary'),
      },
      { name: 'large.txt', mimeType: 'text/plain', buffer: Buffer.alloc(1024 * 1024 + 1, 'x') },
    ])
    await page.getByRole('button', { name: 'Review upload' }).click()
    await page.getByRole('button', { name: 'Open change request', exact: true }).click()
    await expect(page.getByTestId('proposal-title')).toHaveText('Upload 4 files')
    await page.getByTestId('act-integrate').click()
    const landing = page.getByRole('dialog', { name: 'Land this change request' })
    await landing.getByRole('button', { name: 'Land', exact: true }).click()
    await expect(landing.getByText('Landed', { exact: true })).toBeVisible()
    await landing.getByRole('button', { name: 'Close', exact: true }).click()
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page
      .getByRole('navigation', { name: 'Code', exact: true })
      .getByRole('link', { name: 'Search code', exact: true })
      .click()
    await page.getByRole('searchbox', { name: 'Search text', exact: true }).fill('needle')
    await page.getByRole('button', { name: 'Search', exact: true }).click()
    await expect(
      page.getByRole('status').filter({ hasText: '50 matching lines found so far' }),
    ).toBeVisible()
    await page.getByRole('button', { name: 'Continue searching', exact: true }).click()
    await expect(
      page.getByRole('status').filter({ hasText: '61 matching lines found' }),
    ).toBeVisible()
    await expect(
      page.getByRole('region', { name: 'Matches in src.txt', exact: true }).getByRole('link'),
    ).toHaveCount(60)
    await expect(page.locator('mark').first()).toHaveText(/needle/i)
    await expect(page.getByText('Some files were excluded', { exact: true })).toBeVisible()
    await page
      .getByRole('region', { name: 'Matches in src.txt', exact: true })
      .getByRole('link')
      .last()
      .click()
    await expect(page.locator('[data-line="2000"].target .txt')).toHaveText('needle 1999')
    await expect(page.locator('[data-line="2000"].target')).toBeInViewport()
    await expect(page).toHaveURL(/line=2000/)
    await page.goBack()
    await expect(page.getByRole('searchbox', { name: 'Search text', exact: true })).toHaveValue(
      'needle',
    )
    await page.getByRole('searchbox', { name: 'Search text', exact: true }).fill('Needle')
    await page.getByLabel('File or folder (optional)', { exact: true }).fill('src.txt')
    await page.getByLabel('Match case', { exact: true }).check()
    await page.getByRole('button', { name: 'Search', exact: true }).click()
    await expect(
      page.getByRole('status').filter({ hasText: '1 matching line found' }),
    ).toBeVisible()
    await page.reload()
    await expect(page.getByLabel('Match case', { exact: true })).toBeChecked()
    await expect(
      page.getByRole('status').filter({ hasText: '1 matching line found' }),
    ).toBeVisible()
    await expectAccessible(page, info, 'code search')
    await page.screenshot({ path: info.outputPath('code-search.png'), fullPage: true })
  } finally {
    await page.close()
    await server.stop()
  }
})
