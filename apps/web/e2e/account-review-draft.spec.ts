import { test, expect } from './fixtures.ts'
import { amend, commitFiles, freshRepo, openProposal } from './support/world.ts'

test('private account drafts cross devices without overwriting concurrent work or submitting stale reviews', async ({
  cast,
  openAs,
  allowConsole,
}, info) => {
  const { repo } = await freshRepo(cast, 'account-draft')
  const author = cast.as('alice')
  const original = await commitFiles(
    author,
    repo,
    { 'code.txt': 'first version\n' },
    'Review source',
  )
  const proposal = await openProposal(author, repo, {
    head: original.head,
    title: 'Review from another device',
  })
  const path = `/v1/repos/${repo}/proposals/${proposal.id}/review-draft`
  const url = `/repos/${repo}/proposals/${proposal.id}`
  await cast
    .as('admin')
    .ok_(
      cast.as('admin').post('/v1/roles', { subject: 'user:carol', repo, role: 'reviewer' }),
      'grant isolated reviewer',
    )

  const first = await openAs('bob')
  await first.goto(url)
  await first.getByTestId('act-review').click()
  const firstDialog = first.getByRole('dialog', { name: 'Write a review' })
  await firstDialog
    .getByRole('textbox', { name: 'Comment', exact: true })
    .fill('Reasoning from my first device')
  await firstDialog.getByRole('radio', { name: 'Request changes', exact: true }).check()
  await firstDialog.getByRole('button', { name: 'Save draft and close' }).click()
  await expect(firstDialog).not.toBeVisible()
  const stored = await cast.as('bob').ok_(cast.as('bob').get(path), 'read own draft')
  expect(stored.present).toBe(true)
  expect(stored.body).toBe('Reasoning from my first device')
  expect(
    (
      await author.ok_(
        author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
        'no published review',
      )
    ).reviews,
  ).toEqual([])
  const other = await cast
    .as('carol')
    .ok_(cast.as('carol').get(path), 'other reviewer has separate draft')
  expect(other.present).toBe(false)
  expect(other.body).toBe('')

  const second = await openAs('bob')
  await second.goto(url)
  await second.getByTestId('act-review').click()
  const secondDialog = second.getByRole('dialog', { name: 'Write a review' })
  await expect(secondDialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(
    stored.body,
  )
  await expect(
    secondDialog.getByRole('radio', { name: 'Request changes', exact: true }),
  ).toBeChecked()
  await first.getByTestId('act-review').click()
  await firstDialog
    .getByRole('textbox', { name: 'Comment', exact: true })
    .fill('Newer reasoning from first device')
  await firstDialog.getByRole('button', { name: 'Save draft and close' }).click()
  await expect(firstDialog).not.toBeVisible()
  await secondDialog
    .getByRole('textbox', { name: 'Comment', exact: true })
    .fill('Keep this second-device draft')
  allowConsole(/status of 412.*\/review-draft(?:\?|\])/)
  await secondDialog.getByRole('button', { name: 'Save draft and close' }).click()
  await expect(secondDialog.getByText('Account draft storage', { exact: true })).toBeVisible()
  await expect(secondDialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(
    'Keep this second-device draft',
  )
  await secondDialog.getByRole('button', { name: 'Reload account draft' }).click()
  await expect(secondDialog.getByText('Two drafts', { exact: true })).toBeVisible()
  await secondDialog.getByRole('button', { name: 'Keep this tab draft' }).click()
  await secondDialog.getByRole('button', { name: 'Save draft and close' }).click()
  await expect(secondDialog).not.toBeVisible()

  const third = await openAs('bob')
  await third.goto(url)
  await third.getByTestId('act-review').click()
  const thirdDialog = third.getByRole('dialog', { name: 'Write a review' })
  await expect(thirdDialog.getByRole('textbox', { name: 'Comment', exact: true })).toHaveValue(
    'Keep this second-device draft',
  )
  const changed = await amend(
    author,
    repo,
    original.workspace,
    { 'code.txt': 'new version\n' },
    'Changed after the draft',
  )
  await author.ok_(
    author.call('PATCH', `/v1/repos/${repo}/proposals/${proposal.id}`, {
      new_head: changed.head,
      expected_head: original.head,
    }),
    'advance proposal',
  )
  await expect(thirdDialog.getByText('The proposal changed', { exact: true })).toBeVisible()
  await expect(
    thirdDialog.getByRole('button', { name: 'Submit review', exact: true }),
  ).toBeDisabled()
  await third.screenshot({ path: info.outputPath('account-review-draft.png'), fullPage: true })
  await thirdDialog.getByRole('button', { name: 'I reviewed the updated changes' }).click()
  await thirdDialog.getByRole('button', { name: 'Submit review', exact: true }).click()
  await expect(thirdDialog).not.toBeVisible()
  const reviewed = await author.ok_(
    author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read published review',
  )
  expect(reviewed.reviews).toHaveLength(1)
  expect(reviewed.reviews[0].head).toBe(changed.head)
  expect(reviewed.reviews[0].body).toBe('Keep this second-device draft')
  const cleared = await cast.as('bob').ok_(cast.as('bob').get(path), 'submitted draft cleared')
  expect(cleared.present).toBe(false)
  expect(cleared.version).toBeGreaterThan(stored.version)
  await first.close()
  await second.close()
  await third.close()
})
