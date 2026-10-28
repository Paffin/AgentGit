import { test, expect } from './fixtures.ts'
import { freshRepo } from './support/world.ts'

test('reuses a cancelled task context without reopening it or copying its execution state', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'reuse-task')
  const api = cast.as('admin')
  const base = `/v1/repos/${repo}`
  const source = await api.ok_(
    api.post(`${base}/tasks`, {
      title: 'Fix the parser',
      description: 'Preserve Unicode identifiers.',
      scope: ['src/parser/**'],
    }),
    'create source task',
  )
  await api.ok_(
    api.post(`${base}/tasks/${source.id}/finish`, { state: 'cancelled' }),
    'cancel source task',
  )
  await page.goto(`/repos/${repo}/tasks/${source.id}`)
  await page.getByRole('link', { name: 'Create a task from this one…', exact: true }).click()
  const form = page.getByRole('dialog', { name: 'Create a task from this one', exact: true })
  await expect(form.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue(
    'Fix the parser',
  )
  await expect(form.getByLabel('Description', { exact: true })).toHaveValue(
    'Preserve Unicode identifiers.',
  )
  await expect(form.getByLabel('Scope (path globs)', { exact: true })).toHaveValue('src/parser/**')
  await expect(form.getByLabel('Assigned agent', { exact: true })).toHaveValue('')
  await expect(form.getByText('This creates a separate task', { exact: false })).toBeVisible()
  await page.screenshot({ path: info.outputPath('reuse-task.png'), fullPage: true })
  await form.getByRole('button', { name: 'Cancel', exact: true }).click()
  await expect(form).not.toBeVisible()
  expect((await api.ok_(api.get(`${base}/tasks`), 'list tasks')).tasks).toHaveLength(1)
  await page.goto(`/repos/${repo}/tasks/${source.id}`)
  await page.getByRole('link', { name: 'Create a task from this one…', exact: true }).click()
  await form.getByRole('textbox', { name: 'Title', exact: true }).fill('Parser follow-up')
  await form.getByRole('button', { name: 'Create task', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Parser follow-up', exact: true })).toBeVisible()
  expect(page.url()).not.toContain(source.id)
  const tasks = (await api.ok_(api.get(`${base}/tasks`), 'read both tasks')).tasks
  expect(tasks).toHaveLength(2)
  expect(tasks.find((item: { id: string }) => item.id === source.id).state).toBe('cancelled')
  const created = tasks.find((item: { id: string }) => item.id !== source.id)
  expect(created.state).toBe('open')
  expect(created.scope).toEqual(['src/parser/**'])
  expect(created.depends_on ?? []).toEqual([])
  expect(created.owner ?? '').toBe('')
})
