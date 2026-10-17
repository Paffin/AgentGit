import { expect, test } from './fixtures.ts'

test.describe('agents', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('agentgit.staleAfterSec', '10'))
  })

  test('runs: active ones with their task and heartbeat, and finished ones in the history', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/agents`)
    await expect(page.getByRole('tab', { name: /Runs/ })).toHaveAttribute('aria-selected', 'true')
    const table = page.getByRole('table').first()
    await expect(table.getByRole('row')).toHaveCount(5) // header + four active
    await expect(table).toContainText('Fix the flaky invoice test')
    await expect(table).toContainText('just now')
    await expect(table).toContainText(state.demo.runs['refactor-bot'])

    await page.getByRole('radio', { name: 'History' }).check()
    await expect(table).toContainText('Removed three unused exports from src/billing.')
    await expect(table).toContainText('npm audit fix could not resolve a peer dependency conflict.')
    await expect(table).toContainText('completed')
    await expect(table).toContainText('failed')
  })

  test('tasks: the board groups by state and says what is waiting for what', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/agents`)
    await page.getByRole('tab', { name: /Tasks/ }).click()
    const board = page.getByTestId('task-board')
    await expect(board).toContainText('Extract the session store')
    await expect(board).toContainText('In progress')
    await expect(board).toContainText('Open')
    await expect(board).toContainText('Done')
    await expect(board).toContainText('Migrate sessions to Redis')
    await expect(board).toContainText(/waiting for|depends on|blocked/i)
    await expect(board).toContainText('refactor-bot')
  })

  test('leases: scope, holder, token and time left; expired and released ones are not shown as active', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/agents`)
    await page.getByRole('tab', { name: /Leases/ }).click()
    const table = page.getByRole('table').first()
    await expect(table.getByRole('row')).toHaveCount(6) // header + five
    await expect(table).toContainText('src/auth/**')
    await expect(table).toContainText('exclusive')
    await expect(table).toContainText('soft')
    await expect(table).toContainText(`${state.demo.fencing.auth}`)
    await expect(table).toContainText(/\d+m/)
  })

  test('registered agents: identity, role and public key', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/agents`)
    await page.getByRole('tab', { name: /Registered agents/ }).click()
    const list = page.getByRole('tabpanel')
    for (const name of ['refactor-bot', 'docs-writer', 'test-fixer', 'dep-updater']) {
      await expect(list).toContainText(name)
    }
    await expect(list).toContainText(/[0-9a-f]{12}/)
  })
})
