import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal, integrate } from './support/world.ts'

const base =
  'heading\nshared line one\nshared line two\nshared line three\nvalue=base\nshared line four\nshared line five\nend\n'
const mine = base.replace('value=base', 'value=proposal')
const branch = base.replace('value=base', 'value=branch')

for (const renamed of ['proposal', 'branch']) {
  test(`renamed ${renamed} side preserves both overlapping edits and writes the selected source`, async ({
    cast,
    openAs,
  }, info) => {
    const { repo } = await freshRepo(cast, 'rename-conflict', { 'old.txt': base })
    const alice = cast.as('alice')
    const dana = cast.as('dana')
    const ours = await commitFiles(
      alice,
      repo,
      renamed === 'proposal' ? { 'old.txt': null, 'renamed.txt': mine } : { 'old.txt': mine },
      'Proposal edit',
    )
    const proposal = await openProposal(alice, repo, {
      head: ours.head,
      title: 'Resolve renamed overlapping edits',
    })
    const theirs = await commitFiles(
      dana,
      repo,
      renamed === 'branch' ? { 'old.txt': null, 'renamed.txt': branch } : { 'old.txt': branch },
      'Branch edit',
    )
    const upstream = await openProposal(dana, repo, { head: theirs.head, title: 'Branch update' })
    await integrate(cast.as('carol'), repo, upstream.id)
    const page = await openAs('alice')
    await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
    await page.getByTestId('fix-conflict').click()
    const row = page.getByTestId('resolver').locator('[data-path="renamed.txt"]')
    await expect(row.getByRole('region', { name: 'Your version of renamed.txt' })).toContainText(
      'value=proposal',
    )
    await expect(
      row.getByRole('region', { name: 'The branch’s version of renamed.txt' }),
    ).toContainText('value=branch')
    await expect(row).toContainText('Source: old.txt')
    await expect(row).not.toContainText('deleted this file')
    const chooseBranch = renamed === 'proposal'
    await row
      .getByRole('button', { name: chooseBranch ? 'Keep the branch’s' : 'Keep mine', exact: true })
      .click()
    await page.screenshot({ path: info.outputPath('rename-conflict.png'), fullPage: true })
    await page.getByTestId('resolve-save').click()
    await expect(page.getByRole('dialog')).toHaveCount(0)
    const detail = await alice.ok_(
      alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
      'resolved proposal',
    )
    const expectedHead = chooseBranch ? theirs.head : ours.head
    const source = await alice.ok_(
      alice.get(`/v1/repos/${repo}/tree/resolve?rev=${expectedHead}&path=old.txt`),
      'source identity',
    )
    const result = await alice.ok_(
      alice.get(`/v1/repos/${repo}/tree/resolve?rev=${detail.proposal.head}&path=renamed.txt`),
      'resolved identity',
    )
    expect(result.entry.blob_id).toBe(source.entry.blob_id)
    const old = await alice.ok_(
      alice.get(`/v1/repos/${repo}/tree/resolve?rev=${detail.proposal.head}&path=old.txt`),
      'old path stays absent',
    )
    expect(old.kind).toBe('missing')
    await page.close()
  })

  test(`rename/delete with ${renamed} rename preserves the existing side`, async ({
    cast,
    openAs,
  }) => {
    const { repo } = await freshRepo(cast, 'rename-delete', { 'old.txt': base })
    const alice = cast.as('alice')
    const dana = cast.as('dana')
    const ours = await commitFiles(
      alice,
      repo,
      renamed === 'proposal' ? { 'old.txt': null, 'renamed.txt': base } : { 'old.txt': null },
      'Proposal move or delete',
    )
    const proposal = await openProposal(alice, repo, {
      head: ours.head,
      title: 'Resolve rename and delete',
    })
    const theirs = await commitFiles(
      dana,
      repo,
      renamed === 'branch' ? { 'old.txt': null, 'renamed.txt': base } : { 'old.txt': null },
      'Branch move or delete',
    )
    const upstream = await openProposal(dana, repo, { head: theirs.head, title: 'Branch update' })
    await integrate(cast.as('carol'), repo, upstream.id)
    const page = await openAs('alice')
    await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
    await page.getByTestId('fix-conflict').click()
    const row = page.getByTestId('resolver').locator('[data-path="renamed.txt"]')
    await expect(row).toContainText(
      renamed === 'proposal' ? '(the branch deleted this file)' : '(you deleted this file)',
    )
    await row
      .getByRole('button', {
        name: renamed === 'proposal' ? 'Keep mine' : 'Keep the branch’s',
        exact: true,
      })
      .click()
    await page.getByTestId('resolve-save').click()
    await expect(page.getByRole('dialog')).toHaveCount(0)
    const detail = await alice.ok_(
      alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
      'resolved proposal',
    )
    const result = await alice.ok_(
      alice.get(`/v1/repos/${repo}/tree/resolve?rev=${detail.proposal.head}&path=renamed.txt`),
      'preserved renamed file',
    )
    expect(result.entry.file_type).toBe('regular')
    await page.close()
  })
}
