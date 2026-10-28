import { test, expect } from './fixtures.ts'
import { freshRepo, startRun } from './support/world.ts'

test('saved output survives completion, remains text, and can be removed by an operator', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'run-output')
  const run = await startRun(cast, repo, 'refactor-bot')
  const path = `/v1/repos/${repo}/runs/${run.runId}/output`
  const admin = cast.as('admin')
  await admin.ok_(
    admin.put(path, {
      output: {
        sequence: 1,
        stdout: 'Preparing workspace\n',
        stderr: '<script>window.outputExecuted=true</script>\nBuild failed: missing dependency',
        stdout_bytes: 20,
        stderr_bytes: 78,
      },
    }),
    'save output',
  )
  await run.api.ok_(
    run.as('POST', `/v1/repos/${repo}/runs/${run.runId}/complete`, {
      status: 'failed',
      summary: 'Build command failed',
    }),
    'complete run',
  )
  const url = `/repos/${repo}/runs/${run.runId}`
  await page.goto(url)
  await page.getByRole('tab', { name: 'Standard error', exact: true }).click()
  await expect(page.getByLabel('Captured standard error', { exact: true })).toContainText(
    'Build failed: missing dependency',
  )
  expect(await page.evaluate(() => Object.hasOwn(window, 'outputExecuted'))).toBe(false)
  await page.reload()
  await page.getByRole('tab', { name: 'Standard error', exact: true }).click()
  await expect(page.getByLabel('Captured standard error', { exact: true })).toContainText(
    '<script>',
  )
  await page.screenshot({ path: info.outputPath('saved-output.png'), fullPage: true })
  const reader = await openAs('erin')
  await reader.goto(url)
  await expect(
    reader.getByRole('button', { name: 'Delete saved output…', exact: true }),
  ).toHaveCount(0)
  await reader.close()
  await page.getByRole('button', { name: 'Delete saved output…', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Delete saved executor output?', exact: true })
    .getByRole('button', { name: 'Delete output', exact: true })
    .click()
  await expect(page.getByText('No output saved.', { exact: false })).toBeVisible()
  expect((await admin.get(path)).json.available).toBe(false)
})
