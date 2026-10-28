import { test, expect } from './fixtures.ts'
import { freshRepo, startRun } from './support/world.ts'

test('continues failed work explicitly without reopening the source task or losing failure context', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'run-recovery')
  const api = cast.as('admin')
  const base = `/v1/repos/${repo}`
  const task = await api.ok_(
    api.post(`${base}/tasks`, {
      title: 'Fix parser validation',
      description: 'Preserve Unicode identifiers.',
      scope: ['src/parser/**'],
    }),
    'create source task',
  )
  const run = await startRun(cast, repo, 'refactor-bot', task.id)
  await run.api.ok_(
    run.as('POST', `${base}/runs/${run.runId}/complete`, {
      status: 'failed',
      summary: 'Validation command exited with status 2.',
    }),
    'fail run',
  )
  const url = `/repos/${repo}/runs/${run.runId}`
  await page.goto(url)
  await expect(page.getByText('The task is still active.', { exact: false })).toBeVisible()
  await expect(page.getByRole('link', { name: 'Create follow-up task…', exact: true })).toHaveCount(
    0,
  )
  await api.ok_(api.post(`${base}/tasks/${task.id}/finish`, { state: 'cancelled' }), 'cancel task')
  await page.reload()
  await page.getByRole('link', { name: 'Create follow-up task…', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Create a task from this one', exact: true })
  await expect(dialog.getByLabel('Description', { exact: true })).toHaveValue(
    /Preserve Unicode identifiers\.[\s\S]*Validation command exited with status 2\./,
  )
  await expect(dialog.getByLabel('Description', { exact: true })).not.toHaveValue(
    new RegExp(run.runId),
  )
  await expect(dialog.getByLabel('Scope (path globs)', { exact: true })).toHaveValue(
    'src/parser/**',
  )
  await expect(dialog.getByLabel('Assigned agent', { exact: true })).toHaveValue('')
  await page.screenshot({ path: info.outputPath('run-recovery.png'), fullPage: true })
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect((await api.ok_(api.get(`${base}/tasks`), 'tasks after cancel')).tasks).toHaveLength(1)
  await page.goto(url)
  await page.getByRole('link', { name: 'Create follow-up task…', exact: true }).click()
  await dialog
    .getByRole('textbox', { name: 'Title', exact: true })
    .fill('Continue parser validation')
  await dialog.getByRole('button', { name: 'Create task', exact: true }).click()
  await expect(
    page.getByRole('heading', { name: 'Continue parser validation', exact: true }),
  ).toBeVisible()
  const tasks = (await api.ok_(api.get(`${base}/tasks`), 'read tasks')).tasks
  expect(tasks).toHaveLength(2)
  expect(tasks.find((item: { id: string }) => item.id === task.id).state).toBe('cancelled')
  const created = tasks.find((item: { id: string }) => item.id !== task.id)
  expect(created.description).toContain('Validation command exited with status 2.')
  expect(created.description).not.toContain(run.runId)
  expect(created.state).toBe('open')
  expect(created.owner || '').toBe('')
  expect(created.depends_on || []).toEqual([])
  expect(created.source_task).toBe(task.id)
  expect(created.source_run).toBe(run.runId)
  await page.reload()
  const origin = page.getByRole('region', { name: 'Source of this task', exact: true })
  await expect(
    origin.getByRole('link', { name: 'Fix parser validation', exact: true }),
  ).toHaveAttribute('href', new RegExp(`/tasks/${task.id}$`))
  await expect(origin.getByRole('link', { name: 'View source run', exact: true })).toHaveAttribute(
    'href',
    new RegExp(`/runs/${run.runId}$`),
  )
  await origin.getByRole('link', { name: 'Fix parser validation', exact: true }).click()
  const followUps = page.getByRole('region', { name: 'Follow-up tasks', exact: true })
  await expect(
    followUps.getByRole('link', { name: 'Continue parser validation', exact: true }),
  ).toBeVisible()
  await followUps.getByRole('link', { name: 'Continue parser validation', exact: true }).click()
  await expect(origin).toBeVisible()
  await page.screenshot({ path: info.outputPath('task-lineage.png'), fullPage: true })
})

test('does not combine an unrelated run with a task template', async ({ page, cast }) => {
  const { repo } = await freshRepo(cast, 'wrong-run-source')
  const api = cast.as('admin')
  const base = `/v1/repos/${repo}`
  const task = await api.ok_(api.post(`${base}/tasks`, { title: 'Unrelated task' }), 'create task')
  const run = await startRun(cast, repo, 'refactor-bot')
  await run.api.ok_(
    run.as('POST', `${base}/runs/${run.runId}/complete`, {
      status: 'failed',
      summary: 'Do not copy this unrelated failure.',
    }),
    'fail unrelated run',
  )
  await page.goto(`/repos/${repo}/agents?tab=tasks&from_task=${task.id}&from_run=${run.runId}`)
  await expect(page.getByText('Cannot use this run', { exact: true })).toBeVisible()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  expect((await api.ok_(api.get(`${base}/tasks`), 'unchanged tasks')).tasks).toHaveLength(1)
})
