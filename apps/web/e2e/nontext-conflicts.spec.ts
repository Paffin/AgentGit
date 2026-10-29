import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal, integrate } from './support/world.ts'
import type { Cast } from './support/world.ts'

async function conflict(cast: Cast) {
  const { repo } = await freshRepo(cast, 'nontext-conflict', {
    'asset.bin': new Uint8Array([0, 1]),
    launch: 'base\n',
  })
  const alice = cast.as('alice'),
    dana = cast.as('dana')
  const ours = await commitFiles(
    alice,
    repo,
    { 'asset.bin': new Uint8Array([0, 2]), launch: 'local\n' },
    'Change asset and launcher',
  )
  const mine = await alice.ok_(
    alice.post(`/v1/repos/${repo}/workspaces/${ours.workspace}/changes`, {
      changes: [
        {
          path: 'asset.bin',
          op: 'put',
          content: Buffer.from([0, 2]).toString('base64'),
          executable: true,
        },
      ],
      expected_head: ours.head,
      message: 'Keep asset executable',
    }),
    'set exact local mode',
  )
  const proposal = await openProposal(alice, repo, {
    head: mine.head,
    title: 'Review non-text conflicts',
  })
  const other = await commitFiles(
    dana,
    repo,
    { 'asset.bin': new Uint8Array([0, 3]) },
    'Upstream asset',
  )
  const branch = await dana.ok_(
    dana.post(`/v1/repos/${repo}/workspaces/${other.workspace}/changes`, {
      changes: [
        {
          path: 'launch',
          op: 'put',
          content: Buffer.from('branch-target').toString('base64'),
          file_type: 'symlink',
        },
      ],
      expected_head: other.head,
      message: 'Launcher becomes a link',
    }),
    'set upstream link',
  )
  const upstream = await openProposal(dana, repo, {
    head: branch.head,
    title: 'Land upstream versions',
  })
  await integrate(cast.as('carol'), repo, upstream.id)
  return { repo, alice, dana, mine, branch, proposal }
}

test('an author keeps exact binary and symlink versions without a text round trip', async ({
  cast,
  openAs,
}, info) => {
  const { repo, alice, mine, branch, proposal } = await conflict(cast)
  const sourceAsset = await alice.ok_(
    alice.get(`/v1/repos/${repo}/tree/resolve?rev=${mine.head}&path=asset.bin`),
    'read local asset identity',
  )
  const sourceLink = await alice.ok_(
    alice.get(`/v1/repos/${repo}/tree/resolve?rev=${branch.head}&path=launch`),
    'read branch link identity',
  )
  const page = await openAs('alice')
  await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
  await page.getByTestId('fix-conflict').click()
  const resolver = page.getByTestId('resolver')
  await expect(resolver.locator('[data-path="asset.bin"]')).toBeVisible()
  await expect(page.getByTestId('resolve-save')).toBeDisabled()
  await expect(resolver.locator('textarea')).toHaveCount(0)
  await resolver
    .locator('[data-path="asset.bin"]')
    .getByRole('button', { name: 'Keep mine', exact: true })
    .click()
  await resolver
    .locator('[data-path="launch"]')
    .getByRole('button', { name: 'Keep the branch’s', exact: true })
    .click()
  await page.screenshot({ path: info.outputPath('nontext-resolution.png'), fullPage: true })
  await page.getByTestId('resolve-save').click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  const detail = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read resolved revision',
  )
  expect(detail.proposal.head).not.toBe(mine.head)
  for (const [path, source] of [
    ['asset.bin', sourceAsset.entry],
    ['launch', sourceLink.entry],
  ]) {
    const resolved = await alice.ok_(
      alice.get(`/v1/repos/${repo}/tree/resolve?rev=${detail.proposal.head}&path=${path}`),
      'read preserved entry',
    )
    expect(resolved.entry).toMatchObject({
      blob_id: source.blob_id,
      file_type: source.file_type,
      executable: source.executable,
    })
  }
  await page.close()
})

test('a branch change keeps selected resolutions and cannot publish a stale merge', async ({
  cast,
  openAs,
  allowConsole,
}) => {
  const { repo, alice, dana, mine, proposal } = await conflict(cast)
  const page = await openAs('alice')
  await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
  await page.getByTestId('fix-conflict').click()
  const resolver = page.getByTestId('resolver')
  await resolver
    .locator('[data-path="asset.bin"]')
    .getByRole('button', { name: 'Keep mine', exact: true })
    .click()
  await resolver
    .locator('[data-path="launch"]')
    .getByRole('button', { name: 'Keep the branch’s', exact: true })
    .click()
  const next = await commitFiles(
    dana,
    repo,
    { 'later.txt': 'new upstream work' },
    'Advance branch after inspection',
  )
  const upstream = await openProposal(dana, repo, { head: next.head, title: 'Later upstream work' })
  await integrate(cast.as('carol'), repo, upstream.id)
  allowConsole(/status of 412.*\/workspaces\/[^/]+\/sync\]$/)
  await page.getByTestId('resolve-save').click()
  await expect(page.getByRole('dialog')).toContainText(
    'Your resolution has not replaced the newer changes',
  )
  await expect(page.getByTestId('resolve-save')).toBeDisabled()
  const current = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read retained proposal',
  )
  expect(current.proposal.head).toBe(mine.head)
  await page.close()
})
