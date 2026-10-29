import { test, expect } from './fixtures.ts'
import { freshRepo, startRun } from './support/world.ts'

test('changes the attempt policy explicitly and preserves stale drafts', async ({
  page,
  cast,
  allowConsole,
}, info) => {
  allowConsole(/status of 412/)
  const { repo } = await freshRepo(cast, 'task-attempt-limit')
  const api = cast.as('dana')
  const base = `/v1/repos/${repo}/tasks`
  const task = await api.ok_(
    api.post(base, { title: 'Bounded work', max_attempts: 1 }),
    'create task',
  )
  await startRun(cast, repo, 'refactor-bot', task.id)
  const path = `${base}/${task.id}`
  await page.goto(`/repos/${repo}/tasks/${task.id}`)
  await expect(page).toHaveTitle(/Bounded work/)
  const attempts = page.getByRole('region', { name: 'Task attempts' })
  await expect(attempts).toContainText('1 started · Limit: 1')
  await page.getByRole('button', { name: 'Change attempt limit…', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Change attempt limit', exact: true })
  const field = dialog.getByRole('spinbutton', { name: 'Maximum attempts' })
  await expect(field).toBeFocused()
  await field.fill('2')
  expect((await api.ok_(api.get(path), 'read unconfirmed policy')).max_attempts).toBe(1)
  await dialog.getByRole('button', { name: 'Confirm attempt limit', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(attempts).toContainText('1 started · Limit: 2')
  await page.reload()
  await expect(attempts).toContainText('1 started · Limit: 2')
  await page.getByRole('button', { name: 'Change attempt limit…', exact: true }).click()
  await field.fill('3')
  await api.ok_(
    api.patch(`${path}/attempt-limit`, { max_attempts: 4, expected_max_attempts: 2 }),
    'competing policy',
  )
  await dialog.getByRole('button', { name: 'Confirm attempt limit', exact: true }).click()
  await expect(dialog.getByText('Attempt limit was not saved', { exact: true })).toBeVisible()
  await expect(field).toHaveValue('3')
  await expect(
    dialog.getByRole('button', { name: 'Confirm attempt limit', exact: true }),
  ).toBeDisabled()
  expect((await api.ok_(api.get(path), 'read winning policy')).max_attempts).toBe(4)
  await dialog.getByRole('button', { name: 'Keep current limit', exact: true }).click()
  await expect(attempts).toContainText('1 started · Limit: 4')
  await page.getByRole('button', { name: 'Change attempt limit…', exact: true }).click()
  await expect(field).toHaveValue('4')
  await field.fill('0')
  await page.screenshot({ path: info.outputPath('attempt-limit-confirmation.png'), fullPage: true })
  await dialog.getByRole('button', { name: 'Confirm attempt limit', exact: true }).click()
  await expect(attempts).toContainText('1 started · No task-local limit')
  const saved = await api.ok_(api.get(path), 'read final policy')
  expect(saved.attempts_started).toBe(1)
  expect(saved.title).toBe(task.title)
  expect(saved.state).toBe('in_progress')
})

test('limits editing to the creator before execution and operators afterwards', async ({
  cast,
  openAs,
}) => {
  const { repo } = await freshRepo(cast, 'task-attempt-permissions')
  const api = cast.as('alice')
  const task = await api.ok_(
    api.post(`/v1/repos/${repo}/tasks`, { title: 'Writer task' }),
    'create task',
  )
  const page = await openAs('alice')
  await page.goto(`/repos/${repo}/tasks/${task.id}`)
  await page.getByRole('button', { name: 'Change attempt limit…', exact: true }).click()
  await page.getByRole('spinbutton', { name: 'Maximum attempts' }).fill('2')
  await page.getByRole('button', { name: 'Confirm attempt limit', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Task attempts' })).toContainText('Limit: 2')
  await startRun(cast, repo, 'refactor-bot', task.id)
  await page.reload()
  await expect(page.getByText('in_progress', { exact: true })).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Change attempt limit…', exact: true }),
  ).toHaveCount(0)
  const reader = await openAs('erin')
  await reader.goto(`/repos/${repo}/tasks/${task.id}`)
  await expect(reader.getByRole('region', { name: 'Task attempts' })).toContainText(
    '1 started · Limit: 2',
  )
  await expect(
    reader.getByRole('button', { name: 'Change attempt limit…', exact: true }),
  ).toHaveCount(0)
})
