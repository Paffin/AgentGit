import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal } from './support/world.ts'

test('requested reviewers persist, follow the current revision and preserve a conflicting draft', async ({
  cast,
  openAs,
  allowConsole,
}) => {
  const { repo } = await freshRepo(cast, 'review-requests')
  const alice = cast.as('alice')
  const initial = await commitFiles(alice, repo, { 'code.txt': 'first\n' }, 'Review source')
  const proposal = await openProposal(alice, repo, {
    head: initial.head,
    title: 'Ask for a review',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}?tab=reviews`
  const author = await openAs('alice')
  await author.goto(url)
  await author.getByRole('button', { name: 'Request review', exact: true }).click()
  const dialog = author.getByRole('dialog', { name: 'Request reviewers' })
  await dialog.getByRole('textbox', { name: 'Reviewer identities' }).fill('user:bob')
  const before = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read unchanged request',
  )
  expect(before.requested_reviewers).toEqual([])
  await dialog.getByRole('button', { name: 'Save reviewer requests' }).click()
  await expect(dialog).toHaveCount(0)
  await author.reload()
  const requests = author.getByRole('region', { name: 'Requested reviewers' })
  await expect(requests).toContainText('bob')
  await expect(requests).toContainText('Review requested')
  const saved = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read reviewer request',
  )
  expect(saved.proposal.head).toBe(initial.head)
  expect(saved.requested_reviewers).toEqual(['user:bob'])
  expect(saved.reviews).toEqual([])
  await openProposal(alice, repo, { head: initial.head, title: 'No review requested here' })
  const reviewer = await openAs('bob')
  await reviewer.goto(`/repos/${repo}/proposals?reviewer=me`)
  await expect(reviewer.getByRole('checkbox', { name: 'Review requested from me' })).toBeChecked()
  await expect(reviewer.getByRole('link', { name: 'Ask for a review', exact: true })).toBeVisible()
  await expect(
    reviewer.getByRole('link', { name: 'No review requested here', exact: true }),
  ).toHaveCount(0)
  await reviewer.reload()
  await expect(reviewer.getByRole('link', { name: 'Ask for a review', exact: true })).toBeVisible()
  await reviewer.goto(url)
  await expect(reviewer.getByRole('button', { name: 'Request review', exact: true })).toHaveCount(0)
  await cast.as('bob').ok_(
    cast.as('bob').post(`/v1/repos/${repo}/proposals/${proposal.id}/reviews`, {
      decision: 'approve',
      expected_head: initial.head,
    }),
    'approve inspected revision',
  )
  await author.reload()
  await expect(requests).toContainText('Reviewed this revision')
  const next = await commitFiles(alice, repo, { 'code.txt': 'second\n' }, 'New revision')
  await alice.ok_(
    alice.call('PATCH', `/v1/repos/${repo}/proposals/${proposal.id}`, {
      new_head: next.head,
      expected_head: initial.head,
    }),
    'move proposal head',
  )
  await author.reload()
  await expect(requests).toContainText('Review requested')
  await author.getByRole('button', { name: 'Request review', exact: true }).click()
  await dialog.getByRole('textbox', { name: 'Reviewer identities' }).fill('user:carol')
  await alice.ok_(
    alice.call('PUT', `/v1/repos/${repo}/proposals/${proposal.id}/reviewers`, {
      reviewers: ['user:dave'],
      expected_reviewers: ['user:bob'],
    }),
    'concurrent reviewer edit',
  )
  // This click deliberately provokes the server's inspected-set precondition.
  allowConsole(/status of 412.*\/proposals\/[^/]+\/reviewers\]$/)
  await dialog.getByRole('button', { name: 'Save reviewer requests' }).click()
  await expect(dialog).toContainText('requested reviewers changed')
  await expect(dialog.getByRole('textbox', { name: 'Reviewer identities' })).toHaveValue(
    'user:carol',
  )
  const retained = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read winning edit',
  )
  expect(retained.requested_reviewers).toEqual(['user:dave'])
  await author.close()
  await reviewer.close()
})
