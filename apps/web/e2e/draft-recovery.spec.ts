import { resolve } from 'node:path'
import type { Page } from '@playwright/test'
import { test, expect } from './fixtures.ts'
import { startServer, defaultUi, type RunningServer } from './support/server.ts'
import { freshRepo, suffix } from './support/world.ts'

async function signIn(page: Page, server: RunningServer) {
  if (!page.url().startsWith(`${server.baseURL}/signin`))
    await page.goto(`${server.baseURL}/signin`)
  await page.getByLabel('Access token').fill(server.adminToken)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await page.getByRole('button', { name: 'Continue', exact: true }).click()
  await expect(page).not.toHaveURL(/\/signin(?:\?|$)/)
  await expect(page.getByTestId('user-menu')).toContainText('bootstrap-admin')
}
async function propose(page: Page, title: string) {
  await page.getByTestId('propose-change').click()
  await page
    .getByRole('dialog', { name: 'Propose this change' })
    .getByRole('textbox', { name: 'Title', exact: true })
    .fill(title)
  await page.getByTestId('propose-submit').click()
  await expect(page.getByTestId('proposal-title')).toHaveText(title)
}
async function land(page: Page) {
  await page.getByTestId('act-integrate').click()
  const dialog = page.getByRole('dialog', { name: 'Land this change request' })
  await dialog.getByRole('button', { name: 'Land', exact: true }).click()
  await expect(dialog.getByText('Landed', { exact: true })).toBeVisible()
  await expect(page.getByTestId('proposal-state')).toHaveText('landed')
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await expect(dialog).toBeHidden()
}
async function openReadme(page: Page) {
  await page.getByRole('link', { name: 'Code', exact: true }).click()
  await page.getByRole('link', { name: 'README.md', exact: true }).click()
  await expect(page.getByTestId('edit-file')).toBeVisible()
}

test('an empty server user creates content through UI and recovers edits after reload and sign-in', async ({
  openAs,
}, testInfo) => {
  const server = await startServer({ ui: defaultUi(resolve(import.meta.dirname, '../dist')) })
  const page = await openAs(null)
  page.on('dialog', (dialog) => dialog.accept())
  try {
    await signIn(page, server)
    await page.getByTestId('home-new-repo').click()
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(`team/drafts-${suffix()}`)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()
    await page.getByTestId('new-preset-solo').click()
    await page.getByRole('button', { name: 'Next: review' }).click()
    await page.getByTestId('create-repo').click()
    await page.getByRole('link', { name: 'Code', exact: true }).click()
    await page.getByTestId('new-file').click()
    await page.getByLabel('File path').fill('README.md')
    await page.getByLabel('Contents', { exact: true }).fill('Original line\n')
    await propose(page, 'Initial file')
    await land(page)
    await openReadme(page)
    const fileUrl = page.url()
    await page.getByTestId('edit-file').click()
    await page.getByLabel('Contents of README.md').fill('Draft survives reload — Unicode ✓\n')
    const recoveryStarted = Date.now()
    await page.reload()
    await expect(page.getByTestId('edit-file')).toHaveText('Resume draft')
    await page.getByTestId('edit-file').click()
    await expect(page.getByLabel('Contents of README.md')).toHaveValue(
      'Draft survives reload — Unicode ✓\n',
    )
    await expect(page.getByText('Draft recovered', { exact: true })).toBeVisible()
    await expect(page.getByTestId('draft-saved')).toBeVisible()
    await testInfo.attach('recovery-measurement', {
      body: JSON.stringify({
        sampleCount: 1,
        recoveryMs: Date.now() - recoveryStarted,
        actions: ['reload', 'edit'],
        setup: 'empty server; repository and initial content created through UI',
      }),
      contentType: 'application/json',
    })
    // Signing out and back in uses the actual sign-in UI; no token or draft is injected into storage.
    await page.getByTestId('user-menu').click()
    await page.getByRole('menuitem', { name: /sign out/i }).click()
    const leaving = page.getByRole('alertdialog', { name: 'Leave without proposing?' })
    await expect
      .poll(async () => page.url().includes('/signin') || (await leaving.isVisible()))
      .toBe(true)
    if (await leaving.isVisible())
      await leaving.getByRole('button', { name: 'Keep draft and leave' }).click()
    await expect(page).toHaveURL(/\/signin(?:\?|$)/)
    await signIn(page, server)
    await expect(page).toHaveURL(fileUrl)
    await page.getByTestId('edit-file').click()
    await expect(page.getByLabel('Contents of README.md')).toHaveValue(
      'Draft survives reload — Unicode ✓\n',
    )
    await page.screenshot({ path: testInfo.outputPath('recovered-draft.png'), fullPage: true })
    // Delay delivery of a real write response, without replacing its backend result.
    let releaseWrite!: () => void
    let writeReady!: () => void
    let writeDelivered!: () => void
    const held = new Promise<void>((resolve) => {
      releaseWrite = resolve
    })
    const ready = new Promise<void>((resolve) => {
      writeReady = resolve
    })
    const delivered = new Promise<void>((resolve) => {
      writeDelivered = resolve
    })
    let writes = 0
    await page.route('**/v1/repos/**/workspaces/*/changes', async (route) => {
      writes++
      try {
        const response = await route.fetch()
        writeReady()
        await held
        await route.fulfill({ response })
      } finally {
        writeDelivered()
      }
    })
    try {
      await page.getByTestId('propose-change').click()
      const dialog = page.getByRole('dialog', { name: 'Propose this change' })
      await dialog.getByRole('textbox', { name: 'Title', exact: true }).fill('Recovered edit')
      await page.getByTestId('propose-submit').click()
      await ready
      await expect(dialog.getByRole('textbox', { name: 'Title', exact: true })).toBeDisabled()
      await expect(dialog.getByRole('textbox', { name: 'Description (optional)' })).toBeDisabled()
      await expect(page.getByTestId('propose-submit')).toBeDisabled()
      expect(writes).toBe(1)
      await page.screenshot({
        path: testInfo.outputPath('pending-recovered-edit.png'),
        fullPage: true,
      })
      releaseWrite()
      await expect(page.getByTestId('proposal-title')).toHaveText('Recovered edit')
    } finally {
      releaseWrite()
      await delivered
      await page.unroute('**/v1/repos/**/workspaces/*/changes')
    }
    await land(page)
    await openReadme(page)
    await expect(page.getByTestId('file-viewer')).toContainText('Draft survives reload — Unicode ✓')
    await page.reload()
    await expect(page.getByTestId('file-viewer')).toContainText('Draft survives reload — Unicode ✓')
    await page.getByTestId('edit-file').click()
    await expect(page.getByText('Draft recovered', { exact: true })).toHaveCount(0)
  } catch (error) {
    if (!page.isClosed())
      await page
        .screenshot({ path: testInfo.outputPath('failure-before-cleanup.png'), fullPage: true })
        .catch(() => {})
    throw error
  } finally {
    await page.close()
    await server.stop()
  }
})

// Fixture-backed concurrency regression; separate from the clean-server UI acceptance above.

test('a saved edit can be recovered when another writer deletes its file', async ({
  page,
  cast,
}, testInfo) => {
  const { repo, head } = await freshRepo(cast, 'deleted-draft')
  await page.goto(`/repos/${repo}/files?path=README.md`)
  const fileUrl = page.url()
  await page.getByTestId('edit-file').click()
  await page.getByLabel('Contents of README.md').fill('Keep this unfinished work\n')
  const { commitFiles, openProposal, integrate } = await import('./support/world.ts')
  const deletion = await commitFiles(cast.as('alice'), repo, { 'README.md': null }, 'Remove file')
  const removal = await openProposal(cast.as('alice'), repo, {
    head: deletion.head,
    title: 'Remove file',
  })
  await integrate(cast.as('carol'), repo, removal.id)
  page.on('dialog', (dialog) => dialog.accept())
  await page.reload()
  await expect(page.getByLabel('Contents of README.md')).toHaveValue('Keep this unfinished work\n')
  await expect(page.getByText('Draft recovered', { exact: true })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('missing-file-draft.png'), fullPage: true })
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page.getByRole('button', { name: 'Keep draft and leave' }).click()
  await expect(page).not.toHaveURL(fileUrl)
  await expect(page.getByRole('alertdialog')).toHaveCount(0)
  await page.goto(fileUrl)
  await expect(page.getByLabel('Contents of README.md')).toHaveValue('Keep this unfinished work\n')
  await page.getByTestId('propose-change').click()
  await page.getByTestId('propose-submit').click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Update README.md')
  const id = page.url().split('/').pop()!
  const detail = await cast
    .as('dana')
    .ok_(cast.as('dana').get(`/v1/repos/${repo}/proposals/${id}`), 'read recovered proposal')
  const commit = await cast
    .as('dana')
    .ok_(
      cast.as('dana').get(`/v1/repos/${repo}/changeset?rev=${detail.proposal.head}`),
      'read recovered changeset',
    )
  expect(commit.parents).toEqual([head])
  const preview = await cast
    .as('carol')
    .ok_(
      cast.as('carol').post(`/v1/repos/${repo}/proposals/${id}/prepare`),
      'check edit against deletion',
    )
  expect(preview.conflicts.map((conflict: { path: string }) => conflict.path)).toContain(
    'README.md',
  )
})
