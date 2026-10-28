import { test, expect } from './fixtures.ts'
import { freshRepo, startRun, commitFiles, openProposal } from './support/world.ts'

test('a permanent run page follows completion and keeps its changes separate from another session', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'run-details')
  const actor = cast.as('alice')
  const task = await actor.ok_(
    actor.post(`/v1/repos/${repo}/tasks`, {
      title: 'Implement parser validation',
      description: 'Reject invalid input without changing valid results.',
    }),
    'create task',
  )
  const first = await startRun(cast, repo, 'refactor-bot', task.id)
  const one = await commitFiles(
    first.api,
    repo,
    { 'parser.txt': 'first run\n' },
    'First run output',
    { runId: first.runId },
  )
  const proposal = await openProposal(first.api, repo, {
    head: one.head,
    title: 'Parser validation from this run',
    taskId: task.id,
    runId: first.runId,
  })
  const other = await startRun(cast, repo, 'refactor-bot')
  const two = await commitFiles(
    other.api,
    repo,
    { 'unrelated.txt': 'other run\n' },
    'Other output',
    { runId: other.runId },
  )
  await openProposal(other.api, repo, {
    head: two.head,
    title: 'Unrelated change from another run',
    runId: other.runId,
  })
  await page.goto(`/repos/${repo}/agents`)
  await page.locator(`a[href="/repos/${repo}/runs/${first.runId}"]`).click()
  await expect(page.getByRole('heading', { name: 'Run · refactor-bot', exact: true })).toBeVisible()
  await expect(
    page.getByRole('link', { name: 'Implement parser validation', exact: true }),
  ).toBeVisible()
  const url = page.url()
  await first.api.ok_(
    first.as('POST', `/v1/repos/${repo}/runs/${first.runId}/complete`, {
      status: 'failed',
      summary: 'Validation command exited with status 2.',
    }),
    'finish run',
  )
  await expect(
    page.getByText('Validation command exited with status 2.', { exact: true }),
  ).toBeVisible()
  await page.reload()
  await expect(
    page.getByText('Validation command exited with status 2.', { exact: true }),
  ).toBeVisible()
  const results = page.getByRole('region', { name: 'Changes from this session' })
  await expect(
    results.getByRole('link', { name: 'Parser validation from this run', exact: true }),
  ).toBeVisible()
  await expect(
    results.getByRole('link', { name: 'Unrelated change from another run', exact: true }),
  ).toHaveCount(0)
  await page.screenshot({ path: info.outputPath('run-detail.png'), fullPage: true })
  await results.getByRole('link', { name: 'Parser validation from this run', exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`/proposals/${proposal.id}$`))
  await expect(
    page.getByRole('heading', { name: 'Parser validation from this run', exact: true }),
  ).toBeVisible()
  await page.goto(url)
  await page.getByRole('link', { name: "View this run's change requests", exact: true }).click()
  await expect(
    page.getByRole('link', { name: 'Parser validation from this run', exact: true }),
  ).toBeVisible()
  await expect(
    page.getByRole('link', { name: 'Unrelated change from another run', exact: true }),
  ).toHaveCount(0)
  await page.goto(url)
  await expect(
    page.getByText('Validation command exited with status 2.', { exact: true }),
  ).toBeVisible()
})
