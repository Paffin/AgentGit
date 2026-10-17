import { expect, test } from './fixtures.ts'
import { freshRepo } from './support/world.ts'

test.describe('streams', () => {
  test('lists streams with their heads and policy version, and the reflog of the selected one', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/streams`)
    const streams = page.getByRole('table', { name: 'Streams' })
    await expect(streams.getByRole('button', { name: 'main' })).toBeVisible()
    await expect(streams).toContainText('default')
    // Columns: stream, head, version, policy version, creator.
    const row = streams.getByRole('row').filter({ hasText: 'main' })
    await expect(row.getByRole('cell').nth(2)).toHaveText('4')
    await expect(row.getByRole('cell').nth(3)).toHaveText('2')

    const reflog = page.getByRole('table', { name: 'Reflog' })
    // Every integration moved the head once: the import, healthz, the merge of the readme, the greeting.
    await expect(reflog.getByRole('row')).toHaveCount(5) // header + four moves
    await expect(reflog).toContainText('carol')
  })

  test('creating a stream needs the writer role and shows up with its head', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'streams')
    await page.goto(`/repos/${repo}/streams`)
    await page.getByRole('button', { name: 'New stream' }).click()
    const dialog = page.getByRole('dialog', { name: /New stream/ })
    await dialog.getByLabel('Name').fill('release-1')
    await dialog.getByLabel(/Start at/).fill('main')
    await dialog.getByRole('button', { name: 'Create stream' }).click()
    await expect(
      page.getByRole('table', { name: 'Streams' }).getByRole('button', { name: 'release-1' }),
    ).toBeVisible()
    const heads = await cast.as('dana').get(`/v1/repos/${repo}/streams`)
    expect(heads.json.streams.map((s: { name: string }) => s.name).sort()).toEqual([
      'main',
      'release-1',
    ])
  })
})

test.describe('history', () => {
  test('draws the graph with the merge, newest first', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/history`)
    const log = page.getByTestId('history-log')
    const rows = log.getByRole('listitem')
    await expect(rows).toHaveCount(5)
    await expect(rows.first()).toContainText('Friendlier startup message')
    await expect(rows.last()).toContainText('Import the webapp skeleton')
    // The merge commit says so, in words as well as in the drawing.
    const merge = rows.filter({ hasText: 'merge' })
    await expect(merge).toHaveCount(1)
    await expect(merge).toContainText('Document local development')
  })

  test('opens a changeset: parents, message, files and the diff it made', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/history`)
    await page
      .getByTestId('history-log')
      .getByRole('button', { name: /Answer \/healthz/ })
      .click()
    const detail = page.getByTestId('changeset-detail')
    await expect(detail).toContainText('Answer /healthz without a session')
    await expect(detail).toContainText('alice')
    await expect(detail).toContainText('src/server.ts')
    await detail.getByRole('button', { name: /Show the diff/ }).click()
    await expect(detail).toContainText("req.url === '/healthz'")
    // The merge shows both parents.
    await page
      .getByTestId('history-log')
      .getByRole('button', { name: /Integrate prop_/ })
      .click()
    await expect(detail).toContainText(/2 parents|parents/i)
  })

  test('another revision, and an unknown one with a clear message', async ({
    page,
    state,
    allowConsole,
  }) => {
    allowConsole(/status of 404/)
    await page.goto(`/repos/${state.demo.repo}/history`)
    await page.getByLabel('Revision').fill('main~2')
    await page.getByRole('button', { name: 'Show' }).click()
    await expect(page.getByTestId('history-log').getByRole('listitem').first()).toContainText(
      /healthz|Document local development/,
    )
    await page.getByLabel('Revision').fill('no-such-stream')
    await page.getByRole('button', { name: 'Show' }).click()
    await expect(page.getByText(/not found|does not exist|unknown/i).first()).toBeVisible()
  })
})

test.describe('files', () => {
  test('browses directories and reads a file, with sizes', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/files`)
    const listing = page.getByRole('table', { name: 'Directory listing' })
    await expect(listing).toContainText('README.md')
    await expect(listing.getByRole('link', { name: 'src' })).toBeVisible()
    await listing.getByRole('link', { name: 'src' }).click()
    await page
      .getByRole('table', { name: 'Directory listing' })
      .getByRole('link', { name: 'auth' })
      .click()
    await page
      .getByRole('table', { name: 'Directory listing' })
      .getByRole('link', { name: 'session.ts' })
      .click()

    const viewer = page.getByTestId('file-viewer')
    await expect(viewer).toContainText('export function createSession')
    await expect(viewer).toContainText(/\d+(\.\d+)? (B|KB)/)
    await expect(page.getByRole('navigation', { name: 'Path' })).toContainText('session.ts')
    await expect(viewer.getByRole('link', { name: 'File history' })).toBeVisible()
  })

  test('blame is real: each line shows the changeset, author and time that last changed it', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/files?path=README.md`)
    const viewer = page.getByTestId('file-viewer')
    await expect(viewer).toContainText('Local development')
    await viewer.getByLabel('Blame').check()
    const blame = page.getByTestId('blame')
    const table = blame.getByRole('table', { name: /Line-by-line authorship of README\.md/ })
    await expect(table).toBeVisible()
    // The README was written by alice in the import, and dana added the development section.
    await expect(table).toContainText('alice')
    await expect(table).toContainText('dana')
    await expect(table).toContainText('Document local development')
    await expect(table).toContainText('Import the webapp skeleton')
    // dana's ten lines are one block, labelled once.
    const danaBlock = table.locator('td.who').filter({ hasText: 'dana' })
    await expect(danaBlock).toHaveCount(1)
    await expect(danaBlock).toHaveAttribute('rowspan', '10')
    // Every block links to the changeset it names.
    await table.getByRole('link').first().click()
    await expect(page).toHaveURL(/\/history\?cs=/)
    await expect(page.getByTestId('changeset-detail')).toBeVisible()
  })

  test('a path that does not exist says so', async ({ page, state, allowConsole }) => {
    allowConsole(/status of 404/)
    await page.goto(`/repos/${state.demo.repo}/files?path=nope/missing.ts`)
    await expect(page.getByText('Nothing at this path')).toBeVisible()
    await page.getByRole('link', { name: 'Back to the root' }).click()
    await expect(page.getByRole('table', { name: 'Directory listing' })).toBeVisible()
  })

  test('an empty repository has an empty-state, not an error', async ({ page, allowConsole }) => {
    // The server answers a listing of a stream with no commits with 404; the page turns that into an empty state.
    allowConsole(/status of 404/)
    await page.goto('/repos/acme/mobile/files')
    await expect(page.getByText('This stream has no files yet')).toBeVisible()
    await page.goto('/repos/acme/mobile/history')
    await expect(page.getByText('No history yet')).toBeVisible()
  })
})

test.describe('compare', () => {
  test('shows the diff between two revisions, and refuses an unknown one in words', async ({
    page,
    state,
    allowConsole,
  }) => {
    allowConsole(/status of 4\d\d/)
    await page.goto(`/repos/${state.demo.repo}/compare`)
    await expect(page.getByText('Pick two revisions')).toBeVisible()
    await page.getByRole('combobox', { name: 'From', exact: true }).fill('main~3')
    await page.getByRole('combobox', { name: 'To', exact: true }).fill('main')
    await page.getByRole('button', { name: 'Compare' }).click()
    await expect(page.getByText(/files? changed/)).toBeVisible()
    await expect(page.getByRole('region', { name: /Changes to/ }).first()).toBeVisible()

    await page.getByRole('combobox', { name: 'To', exact: true }).fill('no-such-revision')
    await page.getByRole('button', { name: 'Compare' }).click()
    await expect(page.getByRole('alert').filter({ hasText: 'Cannot compare' })).toBeVisible()
  })
})
