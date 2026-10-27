import { test, expect } from './fixtures.ts'
import { commitFiles, freshRepo, integrate, openProposal } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

test('change request filters survive detail navigation without stealing editing focus', async ({
  page,
  cast,
}) => {
  const { repo } = await freshRepo(cast, 'proposal-filters')
  const author = cast.as('alice')
  const change = await commitFiles(
    author,
    repo,
    { 'filter-proof.txt': 'A change to review\n' },
    'Filter context',
  )
  const proposal = await openProposal(author, repo, {
    head: change.head,
    title: 'Preserve review filters',
  })
  await author.ok_(
    author.post(`/v1/repos/${repo}/proposals/${proposal.id}/close`, {}),
    'close filter fixture',
  )
  await page.goto(`/repos/${repo}/proposals`)
  await page.getByRole('radio', { name: 'Closed', exact: true }).check()
  const authorFilter = page.getByRole('textbox', { name: 'Author', exact: true })
  await authorFilter.fill('user:alice')
  await expect.poll(() => new URL(page.url()).searchParams.get('author')).toBe('user:alice')
  await expect(authorFilter).toBeFocused()
  const search = page.getByRole('searchbox', { name: 'Search loaded change requests', exact: true })
  await search.fill('Preserve')
  await expect.poll(() => new URL(page.url()).searchParams.get('q')).toBe('Preserve')
  await expect(search).toBeFocused()
  const filteredUrl = page.url()
  const scroll = await page.evaluate(() => window.scrollY)
  await page.evaluate(() => {
    window.location.hash = 'filter-context'
  })
  await expect(page).toHaveURL(`${filteredUrl}#filter-context`)
  await expect(search).toBeFocused()
  expect(await page.evaluate(() => window.scrollY)).toBe(scroll)
  await page.goBack()
  await expect(page).toHaveURL(filteredUrl)
  await expect(search).toBeFocused()
  const row = page.getByRole('link', { name: 'Preserve review filters', exact: true })
  await row.click()
  await expect(page).toHaveURL(new RegExp(`/proposals/${proposal.id}$`))
  await expect(page.locator('main')).toBeFocused()
  await page.goBack()
  await expect(page).toHaveURL(filteredUrl)
  await expect(page.getByRole('radio', { name: 'Closed', exact: true })).toBeChecked()
  await expect(authorFilter).toHaveValue('user:alice')
  await expect(search).toHaveValue('Preserve')
  await expect(row).toBeVisible()
})

test('editing a symbolic link preserves its file type', async ({ page, cast }) => {
  const { repo, head } = await freshRepo(cast, 'link-edit')
  const api = cast.as('dana')
  await api.ok_(
    api.post(`/v1/repos/${repo}/workspaces`, {
      name: 'link-setup',
      stream: 'main',
      from_rev: head,
    }),
    'link workspace',
  )
  const edited = await api.ok_(
    api.post(`/v1/repos/${repo}/workspaces/link-setup/changes`, {
      message: 'Add link',
      expected_head: head,
      changes: [
        {
          path: 'readme-link',
          op: 'put',
          content: Buffer.from('README.md').toString('base64'),
          file_type: 'symlink',
        },
      ],
    }),
    'create link',
  )
  const setup = await openProposal(api, repo, { head: edited.head, title: 'Add link' })
  await integrate(cast.as('carol'), repo, setup.id)
  await page.goto(`/repos/${repo}/files?path=readme-link`)
  await page.getByTestId('edit-file').click()
  await page.getByLabel('Link target').fill('docs/notes.md')
  await page.getByTestId('propose-change').click()
  await page.getByTestId('propose-submit').click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Update readme-link')
  const id = page.url().split('/').pop()!
  const proposal = await api.ok_(api.get(`/v1/repos/${repo}/proposals/${id}`), 'link proposal')
  const entry = await api.ok_(
    api.get(`/v1/repos/${repo}/tree/resolve?rev=${proposal.proposal.head}&path=readme-link`),
    'link type',
  )
  expect(entry.entry.file_type).toBe('symlink')
})

test('finds files throughout the repository from the keyboard, including for readers', async ({
  cast,
  openAs,
}, testInfo) => {
  const { repo } = await freshRepo(cast, 'finder', {
    'README.md': 'hello\n',
    'deep/nested/find-me.ts': 'export const answer = 42\n',
  })
  const page = await openAs('erin')
  await page.goto(`/repos/${repo}/files`)
  await page.getByRole('heading', { name: 'Files', exact: true }).waitFor()
  await page.keyboard.press('t')
  const finder = page.getByRole('dialog', { name: 'Find a file' })
  await expect(finder).toBeVisible()
  await finder.getByRole('searchbox', { name: 'File name or path' }).fill('nested/find')
  await expect(finder.getByRole('link', { name: 'deep/nested/find-me.ts' })).toBeVisible()
  await expectAccessible(page, testInfo, 'repository file finder')
  await finder.getByRole('searchbox').press('Enter')
  await expect(page.getByTestId('file-viewer')).toContainText('export const answer = 42')
  await expect(page.getByRole('button', { name: 'Rename or move' })).toHaveCount(0)
})

test('selects a non-first file by keyboard and restores finder focus', async ({ cast, openAs }) => {
  const { repo } = await freshRepo(cast, 'finder-keys', {
    'a-first.txt': 'First file\n',
    'b-target.txt': 'The selected second file\n',
    'c-last.txt': 'Last file\n',
  })
  const page = await openAs('erin')
  await page.goto(`/repos/${repo}/files`)
  const opener = page.getByRole('button', { name: 'Find file', exact: true })
  await opener.click()
  const finder = page.getByRole('dialog', { name: 'Find a file' })
  const search = finder.getByRole('searchbox', { name: 'File name or path' })
  await expect(finder.getByRole('link')).toHaveCount(3)
  await search.press('ArrowDown')
  await expect(finder.getByRole('link', { name: 'a-first.txt', exact: true })).toBeFocused()
  await page.keyboard.press('ArrowDown')
  await expect(finder.getByRole('link', { name: 'b-target.txt', exact: true })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(page.getByTestId('file-viewer')).toContainText('The selected second file')
  await expect(finder).not.toBeVisible()

  await opener.click()
  await search.fill('not-present')
  await expect(finder.getByRole('link')).toHaveCount(0)
  await search.press('Enter')
  await expect(finder).toBeVisible()
  await search.fill('')
  await expect(finder.getByRole('link')).toHaveCount(3)
  await search.press('ArrowDown')
  await page.keyboard.press('End')
  await expect(finder.getByRole('link', { name: 'c-last.txt', exact: true })).toBeFocused()
  await page.keyboard.press('Home')
  await page.keyboard.press('ArrowUp')
  await expect(search).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(finder).not.toBeVisible()
  await expect(opener).toBeFocused()
})

test('renames a binary using its exact stored blob and never overwrites an existing destination', async ({
  page,
  cast,
}, testInfo) => {
  const { repo, head } = await freshRepo(cast, 'rename', {
    'old.bin': new Uint8Array([0, 255, 12, 88]),
    'taken.bin': 'do not replace',
  })
  const api = cast.as('dana')
  const original = await api.ok_(
    api.get(`/v1/repos/${repo}/tree/resolve?rev=${head}&path=old.bin`),
    'source metadata',
  )
  await page.goto(`/repos/${repo}/files?path=old.bin`)
  await page.getByRole('button', { name: 'Rename or move' }).click()
  const form = page.getByRole('dialog', { name: 'Rename or move file' })
  await form.getByLabel('New path').fill('taken.bin')
  await form.getByRole('button', { name: 'Open change request' }).click()
  await expect(form.getByRole('alert')).toContainText('already exists')
  await form.getByLabel('New path').fill('assets/new.bin')
  await expectAccessible(page, testInfo, 'rename file')
  await form.getByRole('button', { name: 'Open change request' }).click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Move old.bin to assets/new.bin')
  const id = page.url().split('/').pop()!
  const proposal = await api.ok_(api.get(`/v1/repos/${repo}/proposals/${id}`), 'rename proposal')
  const renamed = await api.ok_(
    api.get(`/v1/repos/${repo}/tree/resolve?rev=${proposal.proposal.head}&path=assets/new.bin`),
    'renamed metadata',
  )
  expect(renamed.entry.blob_id).toBe(original.entry.blob_id)
  const unchanged = await api.ok_(
    api.get(`/v1/repos/${repo}/tree/resolve?rev=main&path=old.bin`),
    'unchanged branch',
  )
  expect(unchanged.entry.blob_id).toBe(original.entry.blob_id)
  await integrate(cast.as('carol'), repo, id)
  expect(
    (await api.ok_(api.get(`/v1/repos/${repo}/tree/resolve?rev=main&path=old.bin`), 'old path'))
      .kind,
  ).toBe('missing')
})

test('deletes a file through a reviewed change without changing the branch immediately', async ({
  page,
  cast,
}) => {
  const { repo } = await freshRepo(cast, 'delete')
  await page.goto(`/repos/${repo}/files?path=README.md`)
  await page.getByRole('button', { name: 'Delete file', exact: true }).click()
  const form = page.getByRole('dialog', { name: 'Delete file', exact: true })
  await expect(form).toContainText('The branch will not change until')
  await form.getByRole('button', { name: 'Open change request' }).click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Delete README.md')
  const id = page.url().split('/').pop()!
  const api = cast.as('dana')
  expect(
    (
      await api.ok_(
        api.get(`/v1/repos/${repo}/tree/resolve?rev=main&path=README.md`),
        'before review',
      )
    ).kind,
  ).not.toBe('missing')
  await integrate(cast.as('carol'), repo, id)
  expect(
    (
      await api.ok_(
        api.get(`/v1/repos/${repo}/tree/resolve?rev=main&path=README.md`),
        'after landing',
      )
    ).kind,
  ).toBe('missing')
})

test('comparison follows browser history and can swap its direction', async ({ page, cast }) => {
  const { repo, head } = await freshRepo(cast, 'compare-history')
  const changed = await commitFiles(
    cast.as('dana'),
    repo,
    { 'README.md': 'A newer version\n' },
    'Second version',
  )
  const proposal = await openProposal(cast.as('dana'), repo, {
    head: changed.head,
    title: 'Second version',
  })
  const landed = await integrate(cast.as('carol'), repo, proposal.id)
  await page.goto(`/repos/${repo}/compare?to=${head}`)
  await expect(page.getByText('README.md', { exact: true })).toBeVisible()
  await page.getByLabel('From', { exact: true }).fill(head)
  await page.getByRole('combobox', { name: 'To', exact: true }).fill(head)
  await page.getByRole('button', { name: 'Compare', exact: true }).click()
  await expect(page.getByText('No differences', { exact: true })).toBeVisible()
  await page.goBack()
  await expect(page.getByText('README.md', { exact: true })).toBeVisible()
  await page.goForward()
  await expect(page.getByText('No differences', { exact: true })).toBeVisible()
  await page.getByLabel('From', { exact: true }).fill(head)
  await page.getByRole('combobox', { name: 'To', exact: true }).fill(landed.new_head)
  await page.getByRole('button', { name: 'Swap revisions' }).click()
  await expect(page.getByLabel('From', { exact: true })).toHaveValue(landed.new_head)
  await expect(page.getByRole('combobox', { name: 'To', exact: true })).toHaveValue(head)
  await expect(page).toHaveURL(new RegExp(`from=${landed.new_head}&to=${head}`))
})

test('uploads binary and text files with an explicit replacement review', async ({
  page,
  cast,
}, testInfo) => {
  const { repo } = await freshRepo(cast, 'upload')
  await page.goto(`/repos/${repo}/files`)
  await page.getByRole('button', { name: 'Upload files', exact: true }).click()
  await page.getByLabel('Files to upload').setInputFiles([
    { name: 'README.md', mimeType: 'text/markdown', buffer: Buffer.from('Uploaded replacement\n') },
    {
      name: 'asset.bin',
      mimeType: 'application/octet-stream',
      buffer: Buffer.from([0, 255, 12, 88]),
    },
  ])
  await page.getByRole('button', { name: 'Review upload' }).click()
  await expect(page.getByText('1 existing file will be replaced.')).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Open change request', exact: true }),
  ).toBeDisabled()
  await page.getByLabel('Replace the existing files listed above').check()
  await expectAccessible(page, testInfo, 'upload review')
  await page.getByRole('button', { name: 'Open change request', exact: true }).click()
  await expect(page.getByTestId('proposal-title')).toHaveText('Upload 2 files')
  const id = page.url().split('/').pop()!
  await integrate(cast.as('carol'), repo, id)
  const response = await page.request.get(`/v1/repos/${repo}/files/asset.bin?rev=main`, {
    headers: { authorization: `Bearer ${cast.token('dana')}` },
  })
  expect(response.ok()).toBe(true)
  expect(await response.body()).toEqual(Buffer.from([0, 255, 12, 88]))
})
