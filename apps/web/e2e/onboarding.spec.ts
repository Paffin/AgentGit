import { rmSync } from 'node:fs'
import { expect, test } from './fixtures.ts'
import { newShell } from './support/cli.ts'
import { expectAccessible } from './support/ui.ts'
import { suffix } from './support/world.ts'

// "Connect your first agent": the commands the panel shows are taken FROM THE PAGE and run, with the real
// `curl` and the real `ag`, against the real server this suite started. If a command on the page were wrong,
// this test would fail.

const PLACEHOLDER = '<token from step 2>'

test.describe('the first-run panel', () => {
  test('shows on a repository nobody has pushed to, and its commands work', async ({
    page,
    cast,
    state,
  }, testInfo) => {
    test.setTimeout(120_000)
    const repo = `acme/first-${suffix()}`
    const admin = cast.as('admin')
    await admin.ok_(admin.post('/v1/repos', { name: repo }), 'create the repository')

    await page.goto(`/repos/${repo}`)
    const panel = page.getByTestId('connect-agent')
    await expect(panel).toBeVisible()
    await expect(panel.getByRole('heading', { name: 'Connect your first agent' })).toBeVisible()
    await expectAccessible(page, testInfo, 'connect panel')

    // Take the commands from the page.
    const role = await page.getByTestId('connect-cmd-role').innerText()
    const credential = await page.getByTestId('connect-cmd-credential').innerText()
    const work = await page.getByTestId('connect-cmd-work').innerText()
    // They name this server and this repository, not placeholders.
    expect(role).toContain(`${state.baseURL}/v1/roles`)
    expect(role).toContain(repo)
    expect(work).toContain(`${state.baseURL}/${repo}`)
    expect(work).toContain(PLACEHOLDER)

    const shell = newShell()
    try {
      const env = { AG_ADMIN_TOKEN: cast.token('admin') }
      // Steps 1 and 2 as an administrator.
      await shell.run(role, env)
      const minted = JSON.parse(await shell.run(credential, env)) as { token: string }
      expect(minted.token).toMatch(/^agt_/)
      // Step 3 as the agent, with the token of step 2 where the page says so.
      const out = await shell.run(work.replace(PLACEHOLDER, minted.token))
      expect(out).toContain('agent:my-agent')
    } finally {
      rmSync(shell.dir, { recursive: true, force: true })
    }

    // The proposal the agent pushed is on the board, and the panel has done its job.
    await expect(page.getByTestId('proposals-panel').getByText('First change')).toBeVisible({
      timeout: 15_000,
    })
    await expect(panel).toHaveCount(0)
  })

  test('refuses an identity it could not put safely into a shell line', async ({ page, cast }) => {
    const repo = `acme/first-${suffix()}`
    const admin = cast.as('admin')
    await admin.ok_(admin.post('/v1/repos', { name: repo }), 'create the repository')
    await page.goto(`/repos/${repo}`)
    const panel = page.getByTestId('connect-agent')
    await panel.getByLabel('Agent identity').fill("agent:x'; rm -rf ~; '")
    await expect(panel.getByText('Use agent:<name>')).toBeVisible()
    await expect(page.getByTestId('connect-cmd-role')).toHaveCount(0)
    await panel.getByLabel('Agent identity').fill('agent:build-bot')
    await expect(page.getByTestId('connect-cmd-role')).toContainText('"subject":"agent:build-bot"')
  })

  test('is not shown on a repository that has history', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByTestId('connect-agent')).toHaveCount(0)
  })

  test('tells a non-administrator to ask for steps 1 and 2', async ({ cast, openAs }) => {
    const repo = `acme/first-${suffix()}`
    const admin = cast.as('admin')
    await admin.ok_(admin.post('/v1/repos', { name: repo }), 'create the repository')
    const alice = await openAs('alice')
    await alice.goto(`/repos/${repo}`)
    await expect(alice.getByTestId('connect-agent')).toContainText(
      'Steps 1 and 2 need an administrator',
    )
  })
})
