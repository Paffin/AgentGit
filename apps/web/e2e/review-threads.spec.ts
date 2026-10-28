import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal, amend } from './support/world.ts'

test.use({ persona: 'alice' })
test('line discussions retain revision context, replies and resolution across reloads', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'line-threads')
  const alice = cast.as('alice')
  const change = await commitFiles(
    alice,
    repo,
    { 'message.txt': 'keep this branch\n' },
    'Candidate',
  )
  const proposal = await openProposal(alice, repo, {
    head: change.head,
    title: 'Review the fallback',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}`
  await page.goto(url)
  await page.getByRole('button', { name: 'Discuss new line 1 of message.txt', exact: true }).click()
  const draft = page.getByRole('textbox', { name: 'Start this discussion', exact: true })
  await draft.fill('Why do we need this branch?')
  await page.reload()
  await expect(draft).toHaveValue('Why do we need this branch?')
  await page.getByRole('button', { name: 'Start discussion', exact: true }).click()
  const thread = page.getByRole('article', {
    name: 'Discussion on message.txt line 1',
    exact: true,
  })
  await expect(thread.getByText('Why do we need this branch?', { exact: true })).toBeVisible()
  await expect(thread.locator('pre')).toHaveText('keep this branch')
  await expect(
    thread.getByRole('button', { name: 'Resolve discussion', exact: true }),
  ).toBeDisabled()
  const bob = await openAs('bob')
  await bob.goto(`${url}?tab=reviews`)
  const bobThread = bob.getByRole('article', {
    name: 'Discussion on message.txt line 1',
    exact: true,
  })
  await bobThread.getByRole('button', { name: 'Replies and actions', exact: true }).click()
  await bobThread
    .getByRole('textbox', { name: 'Reply to this discussion', exact: true })
    .fill('This keeps recovery available.')
  await bobThread.getByRole('button', { name: 'Post reply', exact: true }).click()
  await expect(bobThread.getByText('This keeps recovery available.', { exact: true })).toBeVisible()
  await page.reload()
  await thread.getByRole('button', { name: 'Replies and actions', exact: true }).click()
  await expect(thread.getByText('This keeps recovery available.', { exact: true })).toBeVisible()
  await expect(
    thread.getByRole('button', { name: 'Resolve discussion', exact: true }),
  ).toBeEnabled()
  await thread.getByRole('button', { name: 'Resolve discussion', exact: true }).click()
  await expect(thread.getByText('Resolved', { exact: true })).toBeVisible()
  const filters = page.getByRole('group', { name: 'Filter line discussions', exact: true })
  await filters.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(thread).toBeHidden()
  await expect(
    page.getByText('No open discussions in the loaded results.', { exact: true }),
  ).toBeVisible()
  await filters.getByRole('button', { name: 'Resolved', exact: true }).click()
  await expect(thread).toBeVisible()
  await filters.getByRole('button', { name: 'All', exact: true }).click()
  await expect(
    thread.getByRole('textbox', { name: 'Reply to this discussion', exact: true }),
  ).toHaveCount(0)
  const next = await amend(
    alice,
    repo,
    change.workspace,
    { 'message.txt': 'updated branch\n' },
    'Update candidate',
  )
  await alice.ok_(
    alice.patch(`/v1/repos/${repo}/proposals/${proposal.id}`, { new_head: next.head }),
    'update head',
  )
  await page.reload()
  await expect(thread.getByText('Earlier head', { exact: true })).toBeVisible()
  await expect(thread.locator('pre')).toHaveText('keep this branch')
  await thread.getByRole('button', { name: 'Reopen discussion', exact: true }).click()
  await expect(thread.getByText('Open', { exact: true })).toBeVisible()
  await thread.getByRole('button', { name: 'Replies and actions', exact: true }).click()
  const replyDraft = thread.getByRole('textbox', { name: 'Reply to this discussion', exact: true })
  await replyDraft.fill('Keep this draft while filtering.')
  await filters.getByRole('button', { name: 'Resolved', exact: true }).click()
  await expect(thread).toBeHidden()
  await filters.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(replyDraft).toHaveValue('Keep this draft while filtering.')
  await expect(filters.getByRole('button', { name: 'Open', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
  await page.screenshot({ path: info.outputPath('line-thread.png'), fullPage: true })
  await thread.getByRole('link', { name: 'View source at this revision', exact: true }).click()
  await expect(page.locator('[data-line="1"].target')).toHaveText('keep this branch')
  await expect(page).toHaveURL(new RegExp(`rev=${change.head}`))
  await page.goto(`${url}?tab=reviews`)
  const reader = await openAs('erin')
  await reader.goto(`${url}?tab=reviews`)
  const readThread = reader.getByRole('article', {
    name: 'Discussion on message.txt line 1',
    exact: true,
  })
  await expect(readThread.getByText('Earlier head', { exact: true })).toBeVisible()
  await expect(
    readThread.getByRole('button', { name: 'Resolve discussion', exact: true }),
  ).toHaveCount(0)
  await readThread.getByRole('button', { name: 'Replies and actions', exact: true }).click()
  await expect(
    readThread.getByText('This keeps recovery available.', { exact: true }),
  ).toBeVisible()
  await expect(readThread.getByRole('textbox')).toHaveCount(0)
  await reader.close()
  await bob.close()
})
