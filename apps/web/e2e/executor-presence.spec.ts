import { test, expect } from './fixtures.ts'
import { freshRepo, agentKey } from './support/world.ts'

test('registered keys and a reported executor connection are visibly different', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'executor-presence')
  const admin = cast.as('admin')
  for (const name of ['connected-bot', 'registered-only']) {
    await admin.ok_(
      admin.post(`/v1/repos/${repo}/agents`, { name, public_key: agentKey(name).publicHex }),
      'register agent',
    )
  }
  await admin.ok_(
    admin.put(`/v1/repos/${repo}/agents/connected-bot/executor`, {
      name: 'local-command',
      slots: 2,
      available: 1,
      ttl_secs: 90,
    }),
    'report executor',
  )
  await page.goto(`/repos/${repo}/agents?tab=agents`)
  const connected = page
    .getByText('Executor connected', { exact: true })
    .or(page.getByText('Connected', { exact: true }))
    .filter({ visible: true })
  const missing = page
    .getByText('No recent executor report', { exact: true })
    .or(page.getByText('No recent report', { exact: true }))
    .filter({ visible: true })
  await expect(connected).toBeVisible()
  await expect(missing).toBeVisible()
  await page.screenshot({ path: info.outputPath('executor-connection.png'), fullPage: true })
  await page.reload()
  await expect(connected).toBeVisible()
})
