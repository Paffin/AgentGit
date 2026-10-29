import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal } from './support/world.ts'

test('an administrator requires resolved discussions and an integrator resolves before landing', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'required-discussions')
  await page.goto(`/repos/${repo}/admin`)
  await page
    .getByRole('checkbox', { name: 'Require resolved line discussions', exact: true })
    .check()
  await page.getByTestId('policy-save').click()
  await expect(page.getByText('Policy version 1 saved', { exact: true })).toBeVisible()
  await page.reload()
  await expect(
    page.getByRole('checkbox', { name: 'Require resolved line discussions', exact: true }),
  ).toBeChecked()
  const change = await commitFiles(
    cast.as('alice'),
    repo,
    { 'source.txt': 'explain this line\n' },
    'Candidate',
  )
  const proposal = await openProposal(cast.as('alice'), repo, {
    head: change.head,
    title: 'Discuss before landing',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}`
  const reviewer = await openAs('bob')
  await reviewer.goto(url)
  await reviewer
    .getByRole('button', { name: 'Discuss new line 1 of source.txt', exact: true })
    .click()
  await reviewer
    .getByRole('textbox', { name: 'Start this discussion', exact: true })
    .fill('Please explain the fallback.')
  await reviewer.getByRole('button', { name: 'Start discussion', exact: true }).click()
  await expect(reviewer.getByText('Please explain the fallback.', { exact: true })).toBeVisible()

  await reviewer.getByTestId('act-review').click()
  const review = reviewer.getByRole('dialog', { name: 'Write a review' })
  await review.getByRole('radio', { name: 'Approve', exact: true }).check()
  await review.getByRole('button', { name: 'Submit review', exact: true }).click()
  await expect(review).toHaveCount(0)

  const integrator = await openAs('carol')
  await integrator.goto(url)
  await expect(integrator.getByTestId('blockers')).toContainText(
    '1 open line discussion must be resolved',
  )
  await expect(integrator.getByTestId('next-step')).toContainText('Read and resolve')
  await integrator.getByTestId('act-integrate').click()
  let dialog = integrator.getByRole('dialog', { name: 'Land this change request' })
  await dialog.getByRole('button', { name: 'Land', exact: true }).click()
  await expect(dialog).toContainText('Blocked: the branch was not changed')
  const branch = await cast
    .as('carol')
    .ok_(cast.as('carol').get(`/v1/repos/${repo}/streams/head?stream=main`), 'unchanged branch')
  expect(branch.head).toBe(change.base)
  await integrator.screenshot({ path: info.outputPath('required-discussion.png'), fullPage: true })
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await integrator.getByTestId('fix-reviews').click()
  const thread = integrator.getByRole('article', { name: 'Discussion on source.txt line 1' })
  await thread.getByRole('button', { name: 'Replies and actions', exact: true }).click()
  await expect(
    thread.getByRole('button', { name: 'Resolve discussion', exact: true }),
  ).toBeEnabled()
  await thread.getByRole('button', { name: 'Resolve discussion', exact: true }).click()
  await expect(thread.getByText('Resolved', { exact: true })).toBeVisible()
  await integrator.getByRole('button', { name: 'Check again', exact: true }).click()
  await expect(integrator.getByTestId('blockers')).toHaveCount(0)
  await integrator.getByTestId('act-integrate').click()
  dialog = integrator.getByRole('dialog', { name: 'Land this change request' })
  await dialog.getByRole('button', { name: 'Land', exact: true }).click()
  await expect(dialog.getByText('Landed', { exact: true })).toBeVisible()
  await integrator.close()
  await reviewer.close()
})
