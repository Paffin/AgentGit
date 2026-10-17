import { mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { expect, test } from './fixtures.ts'
import { waitLive } from './support/ui.ts'
import { WEB_ROOT } from './support/state.ts'

// The pictures in docs/guides/web-ui.md. They are taken from the seeded demo world (plausible agents,
// task titles and file paths), at desktop size, and written over the committed ones: `npm run test:e2e`
// refreshes them. This project runs first, before other tests add events to the demo repository.

const out = (name: string) => resolve(WEB_ROOT, '../../docs/assets', name)

test.describe('documentation screenshots', () => {
  test.beforeAll(() => mkdirSync(resolve(WEB_ROOT, '../../docs/assets'), { recursive: true }))
  test.beforeEach(async ({ page }) => {
    // Show the agent that went quiet as quiet, without waiting two minutes for it.
    await page.addInitScript(() => localStorage.setItem('agentgit.staleAfterSec', '10'))
  })

  test('live board', async ({ page, state }) => {
    await page.setViewportSize({ width: 1440, height: 1500 })
    await page.goto(`/repos/${state.demo.repo}`)
    await waitLive(page)
    const proposals = page.getByTestId('proposals-panel')
    await expect(proposals.locator('article[data-proposal]')).toHaveCount(6)
    await expect(proposals.getByText('checking')).toHaveCount(0, { timeout: 20_000 })
    await expect(page.getByTestId('runs-panel').getByText('Quiet', { exact: true })).toBeVisible({
      timeout: 25_000,
    })
    await page.mouse.move(0, 0)
    await page.screenshot({ path: out('ui-board.png') })
  })

  test('live board, dark theme', async ({ page, state }) => {
    await page.addInitScript(() => localStorage.setItem('agentgit.theme', 'dark'))
    await page.setViewportSize({ width: 1440, height: 1100 })
    await page.goto(`/repos/${state.demo.repo}`)
    await waitLive(page)
    await expect(page.getByTestId('proposals-panel').getByText('checking')).toHaveCount(0, {
      timeout: 20_000,
    })
    await expect(page.getByTestId('runs-panel').getByText('Quiet', { exact: true })).toBeVisible({
      timeout: 25_000,
    })
    await page.screenshot({ path: out('ui-board-dark.png') })
  })

  test('proposal detail', async ({ page, state }) => {
    await page.setViewportSize({ width: 1440, height: 1250 })
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    await expect(page.getByTestId('changes-tab')).toContainText('3 files changed')
    await expect(
      page.getByRole('list', { name: /four independent states/ }).getByText('checking'),
    ).toHaveCount(0)
    await expect(page.getByTestId('blockers')).toContainText('fencing token')
    await page.mouse.move(0, 0)
    await page.screenshot({ path: out('ui-proposal.png') })
  })

  test('admin: policy with its version diff', async ({ page, state }) => {
    await page.setViewportSize({ width: 1440, height: 1500 })
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await expect(page.getByTestId('policy-text')).toHaveValue(/protected_paths/)
    await expect(page.getByTestId('policy-diff')).toContainText('protected_paths')
    await page.screenshot({ path: out('ui-admin.png') })
  })

  test('file blame', async ({ page, state }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto(`/repos/${state.demo.repo}/files?path=README.md`)
    await page.getByTestId('file-viewer').getByLabel('Blame').check()
    await expect(page.getByTestId('blame').getByRole('table')).toContainText('dana')
    await page.screenshot({ path: out('ui-blame.png') })
  })
})
