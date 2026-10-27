import { expect, test } from './fixtures.ts'
import { amend, commitFiles, freshRepo, openProposal } from './support/world.ts'

test.use({ persona: 'bob' })

test('keeps a review draft and rejects an author update racing with submission', async ({
  page,
  cast,
  allowConsole,
}) => {
  const { repo } = await freshRepo(cast, 'review-draft')
  const author = cast.as('alice')
  const change = await commitFiles(author, repo, { 'review.txt': 'first version' }, 'First version')
  const proposal = await openProposal(author, repo, {
    head: change.head,
    title: 'Review this change',
  })
  await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
  await page.getByTestId('act-review').click()
  const dialog = page.getByRole('dialog', { name: 'Write a review' })
  const comment = 'I checked the first version, including its error handling.'
  await dialog.getByRole('textbox', { name: 'Comment', exact: true }).fill(comment)
  await page.keyboard.press('Escape')
  await expect(dialog).not.toBeVisible()
  await page.getByTestId('act-review').click()
  await expect(dialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(comment)

  // Change the author's head only after the browser has sent the reviewed head.
  // This exercises the server precondition even if the live UI has not heard the update yet.
  const routePattern = `**/v1/repos/${repo}/proposals/${proposal.id}/reviews`
  await page.route(routePattern, async (route) => {
    expect(route.request().postDataJSON().expected_head).toBe(change.head)
    const newer = await amend(
      author,
      repo,
      change.workspace,
      { 'review.txt': 'second version' },
      'Second version',
    )
    await author.ok_(
      author.call('PATCH', `/v1/repos/${repo}/proposals/${proposal.id}`, { new_head: newer.head }),
      'advance the proposal while review is in flight',
    )
    await route.continue()
  })
  allowConsole(/status of 412/)
  const response = page.waitForResponse((r) =>
    r.url().endsWith(`/proposals/${proposal.id}/reviews`),
  )
  await dialog.getByRole('button', { name: 'Submit review', exact: true }).click()
  expect((await response).status()).toBe(412)
  await expect(
    dialog.getByRole('alert').filter({ hasText: 'The server did not accept the review' }),
  ).toBeVisible()
  await expect(dialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(comment)
  const stored = await author.ok_(
    author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read back reviews',
  )
  expect(stored.reviews).toEqual([])

  await page.unroute(routePattern)
  await expect(dialog.getByText('The proposal changed', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'I reviewed the updated changes' }).click()
  await dialog.getByRole('button', { name: 'Submit review', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await page.getByTestId('act-review').click()
  await expect(dialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue('')
  const accepted = await author.ok_(
    author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read back accepted review',
  )
  expect(accepted.reviews).toHaveLength(1)
  expect(accepted.reviews[0].head).toBe(accepted.proposal.head)
})
