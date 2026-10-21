import { expect, test } from './fixtures.ts'
import { waitLive } from './support/ui.ts'
import { SKELETON, commitFiles, freshRepo, openProposal, startRun } from './support/world.ts'

test.describe('the live board of the demo repository', () => {
  test.beforeEach(async ({ page }) => {
    // The demo's dep-updater never heartbeats; judge "quiet" after ten seconds instead of two minutes.
    await page.addInitScript(() => localStorage.setItem('agentgit.staleAfterSec', '10'))
  })

  test('shows runs with heartbeat age, and warns about the one that went quiet', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await waitLive(page)
    const runs = page.getByTestId('runs-panel')
    await expect(runs.locator('[data-run]')).toHaveCount(4)
    for (const name of ['refactor-bot', 'docs-writer', 'test-fixer', 'dep-updater']) {
      await expect(runs.locator('[data-run]').filter({ hasText: name })).toHaveCount(1)
    }

    // Three agents keep sending heartbeats; their runs stay "Running".
    const bot = runs.locator(`[data-run="${state.demo.runs['refactor-bot']}"]`)
    await expect(bot.getByText('Running')).toBeVisible()
    await expect(bot.getByText(/Heartbeat/)).toBeVisible()
    await expect(bot).not.toContainText('Quiet')

    // The fourth does not: a visible warning with how long it has been silent.
    const quiet = runs.locator(`[data-run="${state.demo.runs['dep-updater']}"]`)
    await expect(quiet.getByText('Quiet', { exact: true })).toBeVisible({ timeout: 25_000 })
    await expect(quiet).toContainText(/silent for .* it may be stuck/)
    await expect(page.getByText(/1 quiet/).first()).toBeVisible()
  })

  test('shows the lease map with fencing tokens, countdowns and overlaps', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const map = page.getByTestId('lease-map')
    for (const scope of ['src/auth/**', 'src/billing/**', '.github/**', 'docs/api/**']) {
      await expect(map.getByText(scope, { exact: true }).first()).toBeVisible()
    }
    // Exclusive leases carry the fencing token an integrator must present.
    await expect(map.getByText(`token ${state.demo.fencing.auth}`)).toBeVisible()
    await expect(map.getByText(`token ${state.demo.fencing.billing}`)).toBeVisible()
    await expect(map.getByText('soft').first()).toBeVisible()
    // A countdown, not a timestamp.
    await expect(map.getByText(/expires in \d+m/).first()).toBeVisible()
    // alice's soft lease on src/** overlaps two exclusive ones: shown, with who holds them.
    await expect(map.getByText(/Overlaps/).first()).toBeVisible()
    await expect(map.getByText(/held by agent:refactor-bot, agent:test-fixer/)).toBeVisible()
  })

  test('shows the task graph with dependencies and what is blocked', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const graph = page.getByTestId('task-graph')
    await expect(graph.locator('[data-task]')).toHaveCount(8) // seven open tasks and the finished import
    const redis = graph.locator(`[data-task="${state.demo.tasks.redis}"]`)
    await expect(redis).toContainText('Blocked')
    await expect(redis).toContainText('1 to wait')
    await expect(redis).toHaveAccessibleName(
      /Migrate sessions to Redis.*waiting for Extract the session store/,
    )
    const runbook = graph.locator(`[data-task="${state.demo.tasks.runbook}"]`)
    await expect(runbook).toContainText('2 to wait') // waits for the Redis migration and the metrics

    await redis.click()
    const dialog = page.getByRole('dialog', { name: 'Migrate sessions to Redis' })
    await expect(dialog).toContainText('Extract the session store')
    await page.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)
    await expect(redis).toBeFocused()
  })

  test('shows the merge queue and the conflict predictions', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const queue = page.getByTestId('queue-panel')
    await expect(queue).toContainText('Document how sessions are created')
    await expect(queue.getByText('queued', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Run queue' })).toBeVisible()

    const predictions = page.getByTestId('overlap-panel')
    await expect(page.getByText(/\d+ overlaps/).first()).toBeVisible()
    // Two proposals that edit the same test file, and a proposal that edits files in someone's lease.
    await expect(predictions).toContainText('tests/invoice.test.ts')
    await expect(predictions).toContainText('edit the same files')
    await expect(predictions).toContainText('edits files inside a lease held by someone else')
    // The stream moved under a proposal: the conflict that proposal 6 shows in detail.
    await expect(predictions).toContainText(
      'changed files this proposal also edits since it was opened',
    )
  })

  test('lists open proposals with four independent states each', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const panel = page.getByTestId('proposals-panel')
    await expect(panel.locator('article[data-proposal]')).toHaveCount(6)
    const docs = panel.locator(`article[data-proposal="${state.demo.proposals.docs}"]`)
    for (const name of ['mergeable', 'validated', 'approved', 'integrated']) {
      await expect(docs.locator(`[data-state="${name}"]`)).toHaveCount(1)
    }
    await expect(docs).toContainText('Nothing is in the way. It can land now.')
    const formal = panel.locator(`article[data-proposal="${state.demo.proposals.formal}"]`)
    await expect(formal).toContainText('Conflicting paths')
    await expect(formal).toContainText('src/index.ts')
    // The stale approval is called out instead of being counted.
    const rounding = panel.locator(`article[data-proposal="${state.demo.proposals.rounding}"]`)
    await expect(rounding).toContainText('1 stale approval')
    // Recently landed work is listed below.
    await expect(page.getByText('Recently landed')).toBeVisible()
    await expect(page.getByRole('link', { name: 'Friendlier startup message' })).toBeVisible()
  })

  test('streams the activity feed in plain language and hides housekeeping by default', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    const feed = page.getByTestId('activity-feed')
    await expect(feed).toContainText(/queued\s*prop_\w+\s*for main/)
    await expect(feed).toContainText('finished validation job (passed)')
    const toggle = page.getByRole('checkbox', { name: /Housekeeping/ })
    await expect(toggle).not.toBeChecked()
    await expect(page.getByText(/\d+ hidden/)).toBeVisible()
    await toggle.check()
    await expect(page.getByText(/\d+ hidden/)).toHaveCount(0)
  })
})

test.describe('the board follows the repository live', () => {
  test('a proposal, a run, a lease, a task and a queue entry appear without reloading', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'live')
    await page.goto(`/repos/${repo}`)
    await waitLive(page)
    await page.evaluate(() => ((window as unknown as { __kept: number }).__kept = 1))

    const proposals = page.getByTestId('proposals-panel')
    await expect(proposals).toContainText('No open change requests')
    await expect(page.getByTestId('runs-panel')).toContainText('No agent is running')
    await expect(page.getByTestId('lease-map')).toContainText(/No active reservations/i)

    // 1. someone opens a proposal through the API
    const alice = cast.as('alice')
    const c = await commitFiles(
      alice,
      repo,
      { 'src/util.ts': 'export const triple = (n: number) => n * 3\n' },
      'Add triple',
    )
    const p = await openProposal(alice, repo, { head: c.head, title: 'Add a triple helper' })
    await expect(proposals.locator(`article[data-proposal="${p.id}"]`)).toBeVisible()
    await expect(proposals).toContainText('Add a triple helper')
    await expect(page.getByTestId('board-summary')).toContainText('Open change requests')

    // 2. an agent starts a run on a task and takes a lease
    const task = await alice.ok_(
      alice.post(`/v1/repos/${repo}/tasks`, {
        title: 'Speed up the util module',
        scope: ['src/**'],
      }),
      'task',
    )
    await expect(page.getByTestId('task-graph').locator(`[data-task="${task.id}"]`)).toBeVisible()
    const run = await startRun(cast, repo, 'test-fixer', task.id)
    const runs = page.getByTestId('runs-panel')
    await expect(runs.locator(`[data-run="${run.runId}"]`)).toContainText('test-fixer')
    await expect(page.getByTestId('task-graph').locator(`[data-task="${task.id}"]`)).toContainText(
      'In progress',
    )

    const leased = await run.as('POST', `/v1/repos/${repo}/leases`, {
      scope: 'src/util.ts',
      exclusive: true,
      ttl_ms: 600_000,
      task_id: task.id,
    })
    expect(leased.ok).toBe(true)
    const map = page.getByTestId('lease-map')
    await expect(map.getByText('src/util.ts')).toBeVisible()
    await expect(map.getByText(`token ${leased.json.lease.fencing_token}`)).toBeVisible()
    await expect(runs.locator(`[data-run="${run.runId}"]`)).toContainText('src/util.ts')

    // The proposal now edits a file inside someone else's lease: the blocker shows up on its card.
    await expect(proposals.locator(`article[data-proposal="${p.id}"]`)).toContainText(
      'exclusive reservation on src/util.ts',
    )

    // 3. a soft lease from a person overlaps it
    const soft = await alice.call('POST', `/v1/repos/${repo}/leases`, {
      scope: 'src/**',
      ttl_ms: 600_000,
    })
    expect(soft.ok).toBe(true)
    await expect(map.getByText(/Overlaps/).first()).toBeVisible()

    // 4. the proposal is queued
    await cast
      .as('carol')
      .ok_(cast.as('carol').post(`/v1/repos/${repo}/queue`, { proposal: p.id }), 'enqueue')
    await expect(page.getByTestId('queue-panel')).toContainText('Add a triple helper')

    // 5. the run finishes and the lease is released
    await run.api.ok_(
      run.api.post(`/v1/repos/${repo}/runs/${run.runId}/complete`, {
        status: 'completed',
        summary: 'done',
      }),
      'complete',
    )
    await expect(runs.locator(`[data-run="${run.runId}"]`)).toHaveCount(0)
    await alice.ok_(alice.del(`/v1/repos/${repo}/leases/${soft.json.lease.id}`), 'release')
    await expect(map.getByText('src/**', { exact: true })).toHaveCount(0)

    // Everything above arrived over the event stream: the page was never reloaded.
    expect(await page.evaluate(() => (window as unknown as { __kept?: number }).__kept)).toBe(1)
    await expect(page.getByTestId('activity-feed')).toContainText('opened proposal')
  })

  test('survives an unreachable event stream: shows it, reconnects and catches up on what it missed', async ({
    page,
    cast,
    allowConsole,
  }) => {
    allowConsole(/ERR_CONNECTION_REFUSED|ERR_FAILED|ERR_INTERNET_DISCONNECTED/)
    const { repo } = await freshRepo(cast, 'drop')
    // Only the live stream is cut (as a proxy dropping long-lived connections would); everything else works.
    const sse = /\/v1\/repos\/[^/]+\/[^/]+\/events(\?|$)/
    await page.route(sse, (route) => route.abort('connectionrefused'))
    await page.goto(`/repos/${repo}`)
    const live = page.getByTestId('live-indicator')
    await expect(live).toHaveAttribute('data-status', 'reconnecting', { timeout: 20_000 })
    await expect(live).toContainText('Reconnecting')
    await expect(page.getByTestId('proposals-panel')).toContainText('No open change requests')

    // While the stream is down the world moves on.
    const alice = cast.as('alice')
    const c = await commitFiles(
      alice,
      repo,
      { 'docs/notes.md': '# Notes\n\nWritten while you were away.\n' },
      'Notes',
    )
    const p = await openProposal(alice, repo, { head: c.head, title: 'Written while offline' })
    await expect(page.getByTestId('proposals-panel')).not.toContainText('Written while offline')

    await page.unroute(sse)
    await expect(live).toHaveAttribute('data-status', 'live', { timeout: 40_000 })
    await expect(
      page.getByTestId('proposals-panel').locator(`article[data-proposal="${p.id}"]`),
    ).toBeVisible({ timeout: 20_000 })
  })

  test('a repository with nothing going on shows calm empty states, not spinners or errors', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'quiet', SKELETON)
    await page.goto(`/repos/${repo}`)
    await waitLive(page)
    await expect(page.getByTestId('proposals-panel')).toContainText('No open change requests')
    await expect(page.getByTestId('runs-panel')).toContainText('No agent is running')
    await expect(page.getByTestId('task-graph')).toContainText('No open tasks')
    await expect(page.getByTestId('queue-panel')).toContainText(
      /queue is empty|Nothing is waiting|empty/i,
    )
    await expect(page.locator('.spinner, [aria-busy="true"]')).toHaveCount(0)
  })
})
