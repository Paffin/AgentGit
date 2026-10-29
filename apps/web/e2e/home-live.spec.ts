import { test, expect } from './fixtures.ts'
import { commitFiles, freshRepo, openProposal } from './support/world.ts'

test('selected repository work appears and clears through real events before the polling interval', async ({
  page,
  cast,
}, info) => {
  const { repo, head } = await freshRepo(cast, 'home-live')
  const api = cast.as('dana')
  const change = await commitFiles(api, repo, { 'live.txt': 'A live review\n' }, 'Live review', {
    fromRev: head,
  })
  await page.goto('/')
  await page.getByRole('combobox', { name: 'Repository', exact: true }).selectOption(repo)
  await expect(page.getByText(`Live updates for ${repo}`, { exact: false })).toBeVisible()
  await expect(page.getByTestId('home-refresh')).toBeEnabled()
  const title = `Review new work in ${repo}`
  const proposal = await openProposal(api, repo, { head: change.head, title })
  const row = page.getByTestId('attention-item').filter({ hasText: title })
  await expect(row).toBeVisible({ timeout: 5000 })
  await page.screenshot({ path: info.outputPath('home-live.png'), fullPage: true })
  await api.ok_(api.post(`/v1/repos/${repo}/proposals/${proposal.id}/close`, {}), 'close work')
  await expect(row).toHaveCount(0, { timeout: 5000 })
  await page.getByRole('combobox', { name: 'Repository', exact: true }).selectOption('')
  await expect(
    page.getByText('Select a repository for live updates.', { exact: false }),
  ).toBeVisible()
})
