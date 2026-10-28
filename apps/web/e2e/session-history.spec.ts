import { test, expect } from './fixtures.ts'
import { freshRepo, agentKey } from './support/world.ts'

test('session history follows continuation pages and removes the control at the end', async ({
  page,
  cast,
}) => {
  const { repo } = await freshRepo(cast, 'session-pages')
  const api = cast.as('admin')
  const base = `/v1/repos/${repo}`
  await api.ok_(
    api.post(`${base}/agents`, {
      name: 'history-bot',
      public_key: agentKey('history-bot').publicHex,
    }),
    'register agent',
  )
  for (let i = 0; i < 101; i++) {
    const run = await api.ok_(api.post(`${base}/runs`, { agent: 'history-bot' }), 'start session')
    await api.ok_(
      api.post(`${base}/runs/${run.run_id}/complete`, {
        status: 'completed',
        summary: `Finished work ${i}`,
      }),
      'finish session',
    )
  }
  await page.goto(`/repos/${repo}/agents?tab=runs`)
  await page.getByRole('radio', { name: 'History', exact: true }).check()
  const runs = page.getByRole('table', { name: 'Agent runs', exact: true })
  await expect(runs.getByRole('row')).toHaveCount(101)
  await page.getByRole('button', { name: 'Load more sessions', exact: true }).click()
  await expect(runs.getByRole('row')).toHaveCount(102)
  await expect(page.getByRole('button', { name: 'Load more sessions', exact: true })).toHaveCount(0)
  await page.getByRole('radio', { name: 'Active', exact: true }).check()
  await expect(page.getByText('No active agent sessions', { exact: true })).toBeVisible()
})
