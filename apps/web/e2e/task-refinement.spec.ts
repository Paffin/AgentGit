import { test, expect } from './fixtures.ts'
import { freshRepo, startRun } from './support/world.ts'

test('refines queued instructions, preserves a stale draft and keeps executed tasks immutable', async ({
  page,
  cast,
  allowConsole,
}, info) => {
  allowConsole(/status of (400|412)/)
  const { repo } = await freshRepo(cast, 'task-refinement')
  const api = cast.as('dana')
  const base = `/v1/repos/${repo}`
  const original = { title: 'Clarify parser work', description: 'Before', scope: ['src/**'] }
  const task = await api.ok_(api.post(`${base}/tasks`, original), 'create task')
  const path = `${base}/tasks/${task.id}`
  await page.goto(`/repos/${repo}/tasks/${task.id}`)
  await expect(page).toHaveTitle(/Clarify parser work/)
  await page.getByRole('button', { name: 'Edit task…', exact: true }).click()
  const heading = page.getByRole('heading', { name: 'Edit task instructions', exact: true })
  const dialog = page.getByRole('dialog').filter({ has: heading })
  await expect(dialog).toHaveAttribute('aria-labelledby', (await heading.getAttribute('id')) ?? '')
  await dialog.getByRole('textbox', { name: 'Title', exact: true }).fill('Implement Unicode parser')
  await dialog
    .getByRole('textbox', { name: 'Description', exact: true })
    .fill('Accept λ identifiers and preserve byte offsets.')
  await dialog
    .getByRole('textbox', { name: 'Scope (path globs)', exact: true })
    .fill('src/parser/**\ntests/parser/**')
  await dialog.getByRole('textbox', { name: 'Scope (path globs)', exact: true }).fill('[')
  await dialog.getByRole('button', { name: 'Save instructions', exact: true }).click()
  await expect(dialog.getByText('Task was not saved', { exact: true })).toBeVisible()
  await expect(dialog.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue(
    'Implement Unicode parser',
  )
  await dialog
    .getByRole('textbox', { name: 'Scope (path globs)', exact: true })
    .fill('src/parser/**\ntests/parser/**')
  await expect(dialog.getByRole('button', { name: 'Save instructions', exact: true })).toBeEnabled()
  await page.screenshot({ path: info.outputPath('edit-task.png'), fullPage: true })
  await dialog.getByRole('button', { name: 'Save instructions', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(
    page.getByRole('heading', { name: 'Implement Unicode parser', exact: true }),
  ).toBeVisible()
  await page.reload()
  await expect(
    page.getByText('Accept λ identifiers and preserve byte offsets.', { exact: true }),
  ).toBeVisible()
  const current = await api.ok_(api.get(path), 'read edited task')
  expect(current.scope).toEqual(['src/parser/**', 'tests/parser/**'])
  expect(current.owner).toBe(task.owner)
  expect(current.depends_on).toEqual(task.depends_on)
  await page.getByRole('button', { name: 'Edit task…', exact: true }).click()
  await dialog
    .getByRole('textbox', { name: 'Description', exact: true })
    .fill('My unsaved instructions')
  const competing = { title: current.title, description: 'Another edit won', scope: current.scope }
  const expected = { title: current.title, description: current.description, scope: current.scope }
  await api.ok_(api.patch(path, { content: competing, expected }), 'competing edit')
  await dialog.getByRole('button', { name: 'Save instructions', exact: true }).click()
  await expect(dialog.getByText('Task was not saved', { exact: true })).toBeVisible()
  await expect(dialog.getByRole('textbox', { name: 'Description', exact: true })).toHaveValue(
    'My unsaved instructions',
  )
  await expect(
    dialog.getByRole('button', { name: 'Save instructions', exact: true }),
  ).toBeDisabled()
  await page.screenshot({ path: info.outputPath('stale-task-draft.png'), fullPage: true })
  expect((await api.ok_(api.get(path), 'read winning edit')).description).toBe('Another edit won')
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page.getByRole('button', { name: 'Edit task…', exact: true }).click()
  await expect(dialog.getByRole('textbox', { name: 'Description', exact: true })).toHaveValue(
    'Another edit won',
  )
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await startRun(cast, repo, 'refactor-bot', task.id)
  await page.reload()
  await expect(page.getByText('in_progress', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Edit task…', exact: true })).toHaveCount(0)
})

test('does not offer task editing to a reader', async ({ cast, openAs }) => {
  const page = await openAs('erin')
  const { repo } = await freshRepo(cast, 'read-task-refinement')
  const admin = cast.as('admin')
  const task = await admin.ok_(
    admin.post(`/v1/repos/${repo}/tasks`, { title: 'Read only work' }),
    'create task',
  )
  await page.goto(`/repos/${repo}/tasks/${task.id}`)
  await expect(page.getByRole('heading', { name: 'Read only work', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Edit task…', exact: true })).toHaveCount(0)
})
