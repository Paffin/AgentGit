import { test, expect } from './fixtures.ts'
import { amend, commitFiles, freshRepo, openProposal } from './support/world.ts'

test('account comment drafts cross devices, preserve newer work and require head reinspection', async ({
  cast,
  openAs,
  allowConsole,
}, info) => {
  const { repo } = await freshRepo(cast, 'comment-account')
  const author = cast.as('alice')
  const change = await commitFiles(author, repo, { 'code.txt': 'first\n' }, 'Discuss source')
  const proposal = await openProposal(author, repo, {
    head: change.head,
    title: 'Account discussion drafts',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}?tab=reviews`
  const path = `/v1/repos/${repo}/proposals/${proposal.id}/comment-draft`
  const first = await openAs('alice')
  await first.goto(url)
  const firstText = first.getByRole('textbox', { name: 'Add to the discussion', exact: true })
  await firstText.fill('Private reasoning from the first device')
  await first.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(first.getByText('Draft saved to your account', { exact: true })).toBeVisible()
  const stored = await author.ok_(author.get(path), 'read private comment draft')
  expect(stored.body).toBe('Private reasoning from the first device')
  const publicState = await author.ok_(
    author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read shared proposal',
  )
  expect(publicState.reviews).toEqual([])
  const other = await cast.as('bob').ok_(cast.as('bob').get(path), 'read another account draft')
  expect(other.present).toBe(false)

  const second = await openAs('alice')
  await second.goto(url)
  const secondText = second.getByRole('textbox', { name: 'Add to the discussion', exact: true })
  await expect(secondText).toHaveValue(stored.body)
  await firstText.fill('Newer work on first device')
  await first.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect
    .poll(async () => (await author.ok_(author.get(path), 'read newer draft')).body)
    .toBe('Newer work on first device')
  await secondText.fill('Keep second device text')
  allowConsole(/status of 412.*\/comment-draft(?:\?|\])/)
  await second.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(
    second.getByRole('button', { name: 'Reload account draft', exact: true }),
  ).toBeVisible()
  await expect(secondText).toHaveValue('Keep second device text')
  await second.getByRole('button', { name: 'Reload account draft', exact: true }).click()
  await expect(second.getByText('An account draft is available', { exact: true })).toBeVisible()
  await second.getByRole('button', { name: 'Keep tab draft', exact: true }).click()
  await second.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect
    .poll(async () => (await author.ok_(author.get(path), 'read replaced draft')).body)
    .toBe('Keep second device text')

  const next = await amend(author, repo, change.workspace, { 'code.txt': 'second\n' }, 'New head')
  await author.ok_(
    author.patch(`/v1/repos/${repo}/proposals/${proposal.id}`, { new_head: next.head }),
    'advance proposal',
  )
  const third = await openAs('alice')
  await third.goto(url)
  const thirdText = third.getByRole('textbox', { name: 'Add to the discussion', exact: true })
  await expect(thirdText).toHaveValue('Keep second device text')
  await expect(third.getByRole('button', { name: 'Add comment', exact: true })).toBeDisabled()
  await third.screenshot({ path: info.outputPath('account-comment-draft.png'), fullPage: true })
  await third
    .getByRole('button', { name: 'I have inspected the current changes', exact: true })
    .click()
  await third.getByRole('button', { name: 'Add comment', exact: true }).click()
  await expect(thirdText).toHaveValue('')
  await expect
    .poll(async () => (await author.ok_(author.get(path), 'read cleared draft')).present)
    .toBe(false)
  const posted = await author.ok_(
    author.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read posted comment',
  )
  expect(posted.reviews).toHaveLength(1)
  expect(posted.reviews[0].body).toBe('Keep second device text')
  expect(posted.reviews[0].head).toBe(next.head)

  await author.ok_(
    author.post(`/v1/repos/${repo}/proposals/${proposal.id}/close`),
    'close proposal',
  )
  await third.reload()
  await thirdText.fill('Follow-up on a closed proposal')
  await third.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect
    .poll(async () => (await author.ok_(author.get(path), 'closed proposal draft')).body)
    .toBe('Follow-up on a closed proposal')
})
