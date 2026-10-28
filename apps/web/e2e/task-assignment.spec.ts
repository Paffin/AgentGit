import { test, expect } from './fixtures.ts'
import { freshRepo, agentKey } from './support/world.ts'

test('a queued task reserves its chosen agent and can be reassigned before launch', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'task-assignment')
  const admin = cast.as('admin')
  for (const name of ['refactor-bot', 'docs-writer']) {
    await admin.ok_(
      admin.post(`/v1/repos/${repo}/agents`, {
        name,
        public_key: agentKey(name).publicHex,
        role: 'agent',
      }),
      'register agent',
    )
  }
  await page.goto(`/repos/${repo}/agents?tab=tasks`)
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  const create = page.getByRole('dialog', { name: 'New task', exact: true })
  await create
    .getByRole('textbox', { name: 'Title', exact: true })
    .fill('Explain the parser changes')
  await create.getByLabel('Assigned agent', { exact: true }).selectOption('docs-writer')
  await create.getByRole('button', { name: 'Create task', exact: true }).click()
  await expect(create).not.toBeVisible()
  await page.getByRole('link', { name: 'Explain the parser changes', exact: true }).click()
  await page.getByRole('button', { name: 'Choose agent…', exact: true }).click()
  const choose = page.getByRole('dialog', { name: 'Choose an agent', exact: true })
  await expect(choose.getByLabel('Assigned agent', { exact: true })).toHaveValue('docs-writer')
  await choose.getByLabel('Assigned agent', { exact: true }).selectOption('refactor-bot')
  await choose.getByRole('button', { name: 'Save assignment', exact: true }).click()
  await expect(choose).not.toBeVisible()
  await page.reload()
  await page.screenshot({ path: info.outputPath('assigned-task.png'), fullPage: true })
  const id = page.url().split('/').at(-1)!
  const wrong = await admin.post(`/v1/repos/${repo}/runs`, { agent: 'docs-writer', task_id: id })
  expect(wrong.status).toBe(412)
  await admin.ok_(
    admin.post(`/v1/repos/${repo}/runs`, { agent: 'refactor-bot', task_id: id }),
    'launch assigned agent',
  )
  await page.reload()
  await expect(page.getByRole('button', { name: 'Choose agent…', exact: true })).toHaveCount(0)
})
