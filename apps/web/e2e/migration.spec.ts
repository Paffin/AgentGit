import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { execFileSync } from 'node:child_process'
import { expectAccessible } from './support/ui.ts'
import { expect, test } from './fixtures.ts'

function bundle(): Buffer {
  const dir = mkdtempSync(join(tmpdir(), 'ag-migration-'))
  const git = (...args: string[]) => execFileSync('git', ['-C', dir, ...args], { stdio: 'pipe' })
  try {
    git('init', '-b', 'main')
    git('config', 'user.name', 'Migration Test')
    git('config', 'user.email', 'migration@example.invalid')
    writeFileSync(join(dir, 'hello.txt'), 'Hello from the source repository\n')
    git('add', '.')
    git('-c', 'commit.gpgsign=false', 'commit', '-m', 'Original history')
    git('branch', 'feature')
    git('tag', 'v1.0')
    git('bundle', 'create', join(dir, 'source.bundle'), '--all')
    return readFileSync(join(dir, 'source.bundle'))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

test('imports a real Git bundle after reviewing its branches and tags', async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/import')
  await page
    .getByLabel('Source repository URL (optional)')
    .fill('https://gitlab.example/team/project.git')
  await expect(page.getByLabel('Commands to create a Git bundle')).toContainText(
    'git clone --mirror',
  )
  const name = `acme/import-${Date.now()}-${testInfo.workerIndex}`
  await page.getByLabel('New AgentGit repository').fill(name)
  await page.getByLabel('Git bundle', { exact: true }).setInputFiles({
    name: 'source.bundle',
    mimeType: 'application/octet-stream',
    buffer: bundle(),
  })
  await page.getByRole('button', { name: 'Check the import', exact: true }).click()
  await expect(page.getByText('Review before publishing', { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.getByText('Review before publishing', { exact: true })).toBeVisible()
  await page.getByText('Branch and tag mapping', { exact: true }).click()
  await expect(page.getByText('feature', { exact: true })).toBeVisible()
  await expect(page.getByText('v1.0', { exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await expectAccessible(page, testInfo, 'migration review')
  await page.screenshot({ path: testInfo.outputPath('migration-review-phone.png'), fullPage: true })
  await page.getByRole('button', { name: 'Publish repository', exact: true }).click()
  await expect(page.getByText('Your repository is ready', { exact: true })).toBeVisible()
  await page.getByRole('link', { name: 'Open repository', exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`/repos/${name}$`))
})
