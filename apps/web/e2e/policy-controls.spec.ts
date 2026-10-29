import { test, expect } from './fixtures.ts'
import { freshRepo, policyDocument } from './support/world.ts'

test('edits protected approvers and validation timing without losing runner configuration', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'policy-controls')
  const api = cast.as('dana')
  const original = {
    ...policyDocument({ requireTests: false }),
    protected_paths: [
      { glob: 'src/security/**', min_approvals: 2, approvers: ['user:security-*'] },
    ],
    required_validations: [
      {
        check: 'tests',
        max_age_secs: 600,
        run: { argv: ['cargo', 'test'], timeout_secs: 300, memory_mb: 1024, network: false },
      },
    ],
  }
  await api.ok_(
    api.put(`/v1/repos/${repo}/policy`, { stream: 'main', document: JSON.stringify(original) }),
    'set baseline policy',
  )
  const baseline = await api.ok_(
    api.get(`/v1/repos/${repo}/policy?stream=main`),
    'read baseline policy',
  )
  const canonical = JSON.parse(baseline.document)
  await page.goto(`/repos/${repo}/admin`)
  const form = page.getByTestId('policy-form')
  await form.getByRole('textbox', { name: 'Path 1', exact: true }).fill('src/auth/**')
  await form
    .getByRole('textbox', { name: 'Who may approve path 1', exact: true })
    .fill('user:alice, user:bob')
  await form
    .getByRole('spinbutton', { name: 'Maximum result age (seconds)', exact: true })
    .fill('900')
  await form.getByRole('spinbutton', { name: 'Command timeout (seconds)', exact: true }).fill('120')
  await page.screenshot({ path: info.outputPath('policy-controls.png'), fullPage: true })
  await page.getByTestId('policy-save').click()
  await expect(page.getByText('Policy version 2 saved', { exact: true })).toBeVisible()
  const stored = await api.ok_(api.get(`/v1/repos/${repo}/policy?stream=main`), 'read saved policy')
  const document = JSON.parse(stored.document)
  expect(document.protected_paths).toEqual([
    { glob: 'src/auth/**', min_approvals: 2, approvers: ['user:alice', 'user:bob'] },
  ])
  expect(document.required_validations).toEqual([
    {
      ...canonical.required_validations[0],
      max_age_secs: 900,
      run: { ...canonical.required_validations[0]!.run, timeout_secs: 120 },
    },
  ])
  await page.reload()
  await expect(
    page.getByRole('textbox', { name: 'Who may approve path 1', exact: true }),
  ).toHaveValue('user:alice, user:bob')
  await expect(
    page.getByRole('spinbutton', { name: 'Command timeout (seconds)', exact: true }),
  ).toHaveValue('120')
})
