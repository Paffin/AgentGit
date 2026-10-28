import { test, expect } from './fixtures.ts'
import { freshRepo } from './support/world.ts'

test('an operator pauses durable task launches and readers see the decision', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'execution-control')
  await page.goto(`/repos/${repo}/agents`)
  await page.getByRole('button', { name: 'Pause new task runs…', exact: true }).click()
  const pause = page.getByRole('dialog', { name: 'Pause new task runs?', exact: true })
  await pause
    .getByRole('textbox', { name: 'Reason (optional)', exact: true })
    .fill('Updating the executor')
  await pause.getByRole('button', { name: 'Pause new task runs', exact: true }).click()
  await expect(pause).not.toBeVisible()
  await page.reload()
  await expect(page.getByText('New task runs are paused', { exact: true })).toBeVisible()
  await expect(page.getByText('Updating the executor', { exact: true })).toBeVisible()
  await page.screenshot({ path: info.outputPath('execution-paused.png'), fullPage: true })
  const reader = await openAs('erin')
  await reader.goto(`/repos/${repo}/agents`)
  await expect(reader.getByText('New task runs are paused', { exact: true })).toBeVisible()
  await expect(
    reader.getByRole('button', { name: 'Resume task execution…', exact: true }),
  ).toHaveCount(0)
  await reader.close()
  await page.getByRole('button', { name: 'Resume task execution…', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Resume task execution?', exact: true })
    .getByRole('button', { name: 'Resume task execution', exact: true })
    .click()
  await expect(page.getByText('New task runs are allowed', { exact: true })).toBeVisible()
})
