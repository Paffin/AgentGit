import { test, expect } from './fixtures.ts'
import { freshRepo, agentKey } from './support/world.ts'

test.use({ persona: 'admin' })

test('registered keys and a reported executor connection are visibly different', async ({
  page,
  cast,
  openAs,
  allowConsole,
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
  await page.getByRole('button', { name: 'Disable agent connected-bot', exact: true }).click()
  const disable = page.getByRole('dialog', { name: 'Disable agent?', exact: true })
  await expect(disable).toContainText('Running sessions keep working')
  await disable.getByRole('button', { name: 'Disable agent', exact: true }).click()
  await expect(disable).toBeHidden()
  await expect(
    page
      .getByRole('row')
      .filter({ hasText: 'connected-bot' })
      .getByText('New runs disabled', { exact: true })
      .filter({ visible: true }),
  ).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Enable agent connected-bot', exact: true }),
  ).toBeVisible()
  await page.reload()
  await page.getByRole('button', { name: 'Enable agent connected-bot', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Enable agent?', exact: true })
    .getByRole('button', { name: 'Enable agent', exact: true })
    .click()
  await expect(connected).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Disable agent connected-bot', exact: true }),
  ).toBeVisible()

  // Another operator changes the state after this dialog captured its decision.
  allowConsole(/status of 412/)
  await page.getByRole('button', { name: 'Disable agent connected-bot', exact: true }).click()
  await admin.ok_(
    admin.patch(`/v1/repos/${repo}/agents/connected-bot`, {
      disabled: true,
      expected_disabled: false,
    }),
    'disable concurrently',
  )
  await disable.getByRole('button', { name: 'Disable agent', exact: true }).click()
  await expect(disable.getByRole('alert')).toContainText('agent state changed')
  await disable.getByRole('button', { name: 'Keep current state', exact: true }).click()
  await page.getByRole('button', { name: 'Enable agent connected-bot', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Enable agent?', exact: true })
    .getByRole('button', { name: 'Enable agent', exact: true })
    .click()
  await expect(connected).toBeVisible()
  await page.screenshot({ path: info.outputPath('agent-controls.png'), fullPage: true })
  const reader = await openAs('erin')
  await reader.goto(`/repos/${repo}/agents?tab=agents`)
  await expect(reader.getByRole('table', { name: 'Registered agents', exact: true })).toBeVisible()
  await expect(reader.getByRole('button', { name: /^Disable agent / })).toHaveCount(0)
  await reader.close()
})
