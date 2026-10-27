import { readFile } from 'node:fs/promises'
import { expect, test } from './fixtures.ts'
import { freshRepo } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

test('an unfinished edit survives cancelled navigation and can be downloaded before leaving', async ({
  page,
  cast,
}, testInfo) => {
  const { repo } = await freshRepo(cast, 'protect-edit')
  await page.goto(`/repos/${repo}/files?path=README.md`)
  await page.getByTestId('edit-file').click()
  const editor = page.getByTestId('file-editor')
  const contents = 'My unfinished work\nwith another line\n'
  await editor.getByLabel('Contents of README.md').fill(contents)
  await page.getByRole('link', { name: 'Code', exact: true }).click()
  const confirm = page.getByRole('alertdialog', { name: 'Leave without proposing?' })
  await expect(confirm).toBeVisible()
  await expectAccessible(page, testInfo, 'protect unfinished work')
  await confirm.getByRole('button', { name: 'Keep editing' }).click()
  await expect(editor.getByLabel('Contents of README.md')).toHaveValue(contents)
  const download = page.waitForEvent('download')
  await editor.getByRole('button', { name: 'Download draft' }).click()
  const file = await download
  expect(file.suggestedFilename()).toBe('README.md')
  expect(await readFile((await file.path())!, 'utf8')).toBe(contents)
  await editor.getByRole('button', { name: 'Cancel', exact: true }).click()
  await expect(confirm).toBeVisible()
  await confirm.getByRole('button', { name: 'Discard changes' }).click()
  await expect(editor).toHaveCount(0)
  await expect(page.getByTestId('file-viewer')).toContainText('A tiny repository')
})

test('directory filter is clear, local to the folder, and retains navigation', async ({
  page,
  cast,
}, testInfo) => {
  const { repo } = await freshRepo(cast, 'filter-files')
  let release!: () => void
  const repositoryMetadata = new Promise<void>((resolve) => (release = resolve))
  await page.route(`**/v1/repos/${repo}`, async (route) => {
    await repositoryMetadata
    await route.continue()
  })
  try {
    await page.goto(`/repos/${repo}/files`)
    await page.getByRole('searchbox', { name: 'Filter this folder' }).fill('READ')
  } finally {
    release()
  }
  await expect(page.getByRole('link', { name: 'README.md', exact: true })).toBeVisible()
  await expect(page.getByRole('link', { name: 'src', exact: true })).toHaveCount(0)
  await page.getByRole('searchbox', { name: 'Filter this folder' }).fill('does-not-exist')
  await expect(page.getByText('No matching files')).toBeVisible()
  await page.getByRole('button', { name: 'Clear filter' }).click()
  await page.getByRole('link', { name: 'src', exact: true }).click()
  await expect(page.getByRole('link', { name: 'app.ts', exact: true })).toBeVisible()
  await page.setViewportSize({ width: 390, height: 844 })
  await expectAccessible(page, testInfo, 'folder filter on phone')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('a recovered edit retains its original base when another user advances the branch', async ({
  page,
  cast,
}) => {
  const { repo, head } = await freshRepo(cast, 'concurrent-edit')
  await page.goto(`/repos/${repo}/files?path=README.md`)
  await page.getByTestId('edit-file').click()
  await page.getByLabel('Contents of README.md').fill('My version\n')
  const { commitFiles, openProposal, integrate } = await import('./support/world.ts')
  const other = await commitFiles(
    cast.as('alice'),
    repo,
    { 'README.md': 'Another person’s version\n' },
    'Concurrent edit',
  )
  const change = await openProposal(cast.as('alice'), repo, {
    head: other.head,
    title: 'Concurrent edit',
  })
  const landed = await integrate(cast.as('carol'), repo, change.id)
  page.on('dialog', (dialog) => dialog.accept())
  await page.reload()
  await page.getByTestId('edit-file').click()
  await expect(page.getByLabel('Contents of README.md')).toHaveValue('My version\n')
  await expect(
    page.getByText('The branch has moved since this draft started.', { exact: false }),
  ).toBeVisible()
  await page.getByTestId('propose-change').click()
  await page.getByTestId('propose-submit').click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Update README.md')
  const id = page.url().split('/').pop()!
  const detail = await cast
    .as('dana')
    .ok_(cast.as('dana').get(`/v1/repos/${repo}/proposals/${id}`), 'read proposal')
  const commit = await cast
    .as('dana')
    .ok_(
      cast.as('dana').get(`/v1/repos/${repo}/changeset?rev=${detail.proposal.head}`),
      'read edited changeset',
    )
  expect(commit.parents).toEqual([head])
  expect(commit.parents).not.toContain(landed.new_head)
  const preview = await cast
    .as('carol')
    .ok_(
      cast.as('carol').post(`/v1/repos/${repo}/proposals/${id}/prepare`),
      'prepare concurrent edit',
    )
  expect(preview.conflicts.map((conflict: { path: string }) => conflict.path)).toContain(
    'README.md',
  )
})

test('a branch populated during a new-file edit cannot silently become its base', async ({
  page,
  cast,
}) => {
  const { commitFiles, openProposal, integrate, suffix } = await import('./support/world.ts')
  const repo = `acme/empty-race-${suffix()}`
  await cast
    .as('admin')
    .ok_(cast.as('admin').post('/v1/repos', { name: repo }), 'create empty repository')
  await page.goto(`/repos/${repo}/files`)
  await page.getByTestId('new-file').click()
  await page.getByLabel('File path').fill('README.md')
  await page.getByLabel('Contents', { exact: true }).fill('My first file\n')
  await expect(page.getByTestId('propose-change')).toBeEnabled()
  const other = await commitFiles(
    cast.as('alice'),
    repo,
    { 'README.md': 'Someone else’s first file\n' },
    'First file',
  )
  const change = await openProposal(cast.as('alice'), repo, {
    head: other.head,
    title: 'First file',
  })
  await integrate(cast.as('carol'), repo, change.id)
  page.on('dialog', (dialog) => dialog.accept())
  await page.reload()
  await page.getByTestId('new-file').click()
  await expect(page.getByLabel('File path')).toHaveValue('README.md')
  await expect(page.getByLabel('Contents', { exact: true })).toHaveValue('My first file\n')
  await page.getByTestId('propose-change').click()
  await page.getByTestId('propose-submit').click()
  await expect(page.getByRole('alert')).toContainText('branch changed')
  await page.getByRole('button', { name: 'Back', exact: true }).click()
  await expect(page.getByLabel('Contents', { exact: true })).toHaveValue('My first file\n')
  const active = await cast
    .as('dana')
    .ok_(cast.as('dana').get(`/v1/repos/${repo}/proposals?state=open`), 'list proposals')
  expect(active.proposals ?? []).toHaveLength(0)
})
