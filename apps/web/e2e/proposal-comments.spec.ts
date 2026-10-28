import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal, amend } from './support/world.ts'

test.use({ persona: 'alice' })
test('an author discusses an inspected change without review privileges and retains stale drafts', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'discussion')
  const alice = cast.as('alice')
  const change = await commitFiles(alice, repo, { 'message.txt': 'first\n' }, 'First candidate')
  const proposal = await openProposal(alice, repo, {
    head: change.head,
    title: 'Discuss the implementation',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}?tab=reviews`
  await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
  await page.getByRole('button', { name: 'Discuss…', exact: true }).click()
  const text = page.getByRole('textbox', { name: 'Add to the discussion', exact: true })
  await text.fill('I kept the explicit branch so the caller can recover.')
  await page.reload()
  await expect(text).toHaveValue('I kept the explicit branch so the caller can recover.')
  const next = await amend(
    alice,
    repo,
    change.workspace,
    { 'message.txt': 'second\n' },
    'Updated candidate',
  )
  await alice.ok_(
    alice.patch(`/v1/repos/${repo}/proposals/${proposal.id}`, { new_head: next.head }),
    'move candidate',
  )
  await page.reload()
  await expect(page.getByText('The change request has a new head', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Add comment', exact: true })).toBeDisabled()
  await page
    .getByRole('button', { name: 'I have inspected the current changes', exact: true })
    .click()
  await page.getByRole('button', { name: 'Add comment', exact: true }).click()
  await expect(text).toHaveValue('')
  await expect(
    page.getByText('I kept the explicit branch so the caller can recover.', { exact: true }),
  ).toBeVisible()
  const stored = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read discussion',
  )
  expect(stored.reviews).toHaveLength(1)
  expect(stored.reviews[0].decision).toBe('comment')
  expect(stored.reviews[0].head).toBe(next.head)
  expect(stored.reviews[0].reviewer).toBe('user:alice')
  await page.screenshot({ path: info.outputPath('proposal-discussion.png'), fullPage: true })
  const reader = await openAs('erin')
  await reader.goto(url)
  await expect(
    reader.getByText('I kept the explicit branch so the caller can recover.', { exact: true }),
  ).toBeVisible()
  await expect(
    reader.getByRole('textbox', { name: 'Add to the discussion', exact: true }),
  ).toHaveCount(0)
  await reader.close()
  await cast
    .as('admin')
    .ok_(
      cast.as('admin').post(`/v1/repos/${repo}/proposals/${proposal.id}/close`),
      'close proposal',
    )
  await page.reload()
  await text.fill('Closing this approach after discussion.')
  await page.getByRole('button', { name: 'Add comment', exact: true }).click()
  await expect(
    page.getByText('Closing this approach after discussion.', { exact: true }),
  ).toBeVisible()
})
