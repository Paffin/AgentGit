import { expect, test } from './fixtures.ts'
import type { Page } from '@playwright/test'
import { verdicts, waitLive } from './support/ui.ts'
import {
  commitFiles,
  freshRepo,
  openProposal,
  policyDocument,
  setPolicy,
  startRun,
} from './support/world.ts'

const strip = (page: Page) => page.getByRole('list', { name: /four independent states/ })
const open = (page: Page, repo: string, id: string) => page.goto(`/repos/${repo}/proposals/${id}`)

test.describe('a proposal in the demo repository', () => {
  test('conflicts are listed by path, with what to do about them', async ({ page, state }) => {
    await open(page, state.demo.repo, state.demo.proposals.formal)
    await expect(page.getByTestId('proposal-title')).toHaveText('Print a formal startup line')
    await expect(page.getByTestId('proposal-state')).toHaveText('open')
    expect(await verdicts(strip(page))).toMatchObject({
      mergeable: 'failed',
      integrated: 'pending',
    })
    const conflicts = page.getByTestId('conflicts')
    await expect(conflicts).toContainText('Conflicting paths')
    await expect(conflicts).toContainText('src/index.ts')
    await expect(conflicts).toContainText('content conflict')
    await expect(conflicts).toContainText(
      'Nothing guessed: resolve these in the author’s workspace',
    )
  })

  test('the header names the run, the model and the task behind an agent’s change', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.extract)
    const head = page.locator('header.head').first()
    await expect(head).toContainText('agent:refactor-bot')
    await expect(head).toContainText(state.demo.runs['refactor-bot'])
    await expect(head).toContainText('example-coder-large')
    await expect(head).toContainText('Extract the session store')
    await expect(page.getByTestId('blockers')).toContainText('Needs 1 more approval')
    // The remedy is spelled out for the lease blocker, including the fencing token idea.
    await expect(page.getByTestId('blockers')).toContainText(
      'An exclusive lease on src/auth/** is held by agent:refactor-bot',
    )
    await expect(page.getByTestId('blockers')).toContainText('fencing token')
  })

  test('shows a real diff: files, hunks, line numbers, additions and deletions, as text', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.extract)
    const changes = page.getByTestId('changes-tab')
    await expect(changes).toContainText('3 files changed')
    const nav = changes.getByRole('navigation', { name: 'Changed files' })
    await expect(nav.getByRole('link')).toHaveCount(3)
    await expect(nav).toContainText('src/auth/store.ts')
    await expect(nav.getByText('new')).toBeVisible()

    const file = changes.locator('#diff-0')
    await expect(file).toContainText('@@')
    // The new store file is all additions; its code is shown, not interpreted.
    const store = changes.getByRole('region', { name: 'Changes to src/auth/store.ts' })
    await expect(store).toContainText('implements SessionStore')
    await expect(store.locator('.row.k-add').first()).toBeVisible()
    await expect(store.locator('.row.k-del')).toHaveCount(0)
    await expect(changes.locator('.row.k-del').first()).toBeVisible()

    // Jumping to a file from the list moves focus into that file's header.
    await nav.getByRole('link', { name: /session\.ts/ }).click()
    await expect(page.locator('#diff-1 button').first()).toBeFocused()
    // A file can be collapsed and expanded again from the keyboard.
    const toggle = page.locator('#diff-1 button').first()
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')
    await page.keyboard.press('Enter')
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await page.keyboard.press('Enter')
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')
  })

  test('says so when a file is binary, and shows a single-file diff at full width', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.docs)
    const changes = page.getByTestId('changes-tab')
    await expect(changes).toContainText('Binary file: its contents are not shown.')
    await expect(changes.getByRole('navigation', { name: 'Changed files' })).toContainText('binary')

    await open(page, state.demo.repo, state.demo.proposals.formal)
    const diff = page.getByTestId('changes-tab')
    await expect(diff.getByRole('navigation', { name: 'Changed files' })).toHaveCount(0)
    const box = await diff.locator('#diff-0').boundingBox()
    const container = await diff.boundingBox()
    expect(box!.width).toBeGreaterThan(container!.width * 0.9)
  })

  test('the validation tab shows the evidence: who signed what, for which tree and policy', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.extract)
    await page.getByRole('tab', { name: /Validation/ }).click()
    const tab = page.getByTestId('validation-tab')
    await expect(tab).toContainText('tests')
    await expect(tab).toContainText('passed')
    await expect(tab).toContainText('ci')
    await expect(tab).toContainText('Policy version')
    await expect(tab).toContainText(/sha256:[0-9a-f]{12}/)

    await open(page, state.demo.repo, state.demo.proposals.flaky)
    await page.getByRole('tab', { name: /Validation/ }).click()
    await expect(page.getByTestId('validation-tab')).toContainText('failed')
  })

  test('the provenance tab shows what the agent said about itself, marked untrusted, with a verified signature', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.extract)
    await page.getByRole('tab', { name: /Provenance/ }).click()
    const tab = page.getByTestId('provenance-tab')
    await expect(tab).toContainText('example-coder-large')
    await expect(tab).toContainText('Splitting the store out makes the Redis migration')
    await expect(tab).toContainText('issue #214')
    await expect(tab).toContainText('npm test')
    await expect(tab).toContainText(/signature verified/i)
    await expect(tab).toContainText(/written by the agent|not verified|untrusted/i)
  })

  test('the reviews tab shows decisions with the head they were written against', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.flaky)
    await page.getByRole('tab', { name: /Reviews/ }).click()
    const tab = page.getByTestId('reviews-tab')
    await expect(tab).toContainText('bob')
    await expect(tab).toContainText('Requested changes')
    await expect(tab).toContainText('regression test for a fractional quantity')

    await open(page, state.demo.repo, state.demo.proposals.rounding)
    await page.getByRole('tab', { name: /Reviews/ }).click()
    await expect(page.getByTestId('reviews-tab')).toContainText('Some approvals are stale')
  })

  test('the timeline tab tells the story of the proposal in order', async ({ page, state }) => {
    await open(page, state.demo.repo, state.demo.proposals.docs)
    await page.getByRole('tab', { name: /Timeline/ }).click()
    const tab = page.getByTestId('timeline-tab')
    await expect(tab).toContainText(/opened/i)
    await expect(tab).toContainText(/approved/i)
    await expect(tab).toContainText(/queued/i)
  })

  test('a protected path asks for two approvals and says which rule it comes from', async ({
    page,
    state,
  }) => {
    await open(page, state.demo.repo, state.demo.proposals.node)
    await expect(page.getByTestId('blockers')).toContainText(
      'Needs 1 more approval for .github/** (1 of 2)',
    )
    expect(await verdicts(strip(page))).toMatchObject({ validated: 'ok', approved: 'blocked' })
  })
})

test.describe('acting on a proposal', () => {
  test.use({ persona: 'carol' })

  test('closing asks first, and the proposal then shows as closed everywhere', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'close')
    const alice = cast.as('alice')
    const c = await commitFiles(
      alice,
      repo,
      { 'src/app.ts': 'export const answer = 1\n' },
      'Change',
    )
    const p = await openProposal(alice, repo, { head: c.head, title: 'Abandon this' })
    await open(page, repo, p.id)
    await waitLive(page)

    await page.getByTestId('act-close').click()
    const dialog = page.getByRole('alertdialog', { name: 'Close this proposal?' })
    await expect(dialog).toContainText('It will not be integrated')
    await dialog.getByRole('button', { name: 'Keep it open' }).click()
    await expect(page.getByTestId('proposal-state')).toHaveText('open')

    await page.getByTestId('act-close').click()
    await page.getByTestId('confirm-close').click()
    await expect(page.getByTestId('proposal-state')).toHaveText('closed')
    await expect(page.getByTestId('act-integrate')).toHaveCount(0)
    const got = await cast.as('alice').get(`/v1/repos/${repo}/proposals/${p.id}`)
    expect(got.json.proposal.state).toBe('closed')
  })

  test('adding to the queue shows it on the board; running the queue lands it', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'queue')
    const alice = cast.as('alice')
    const c = await commitFiles(alice, repo, { 'docs/notes.md': '# Notes\n\nMore.\n' }, 'Notes')
    const p = await openProposal(alice, repo, { head: c.head, title: 'Extend the notes' })
    await open(page, repo, p.id)
    await page.getByTestId('act-enqueue').click()
    // Wait for the server's answer (the toast), not for any text that merely mentions the queue: the button does.
    await expect(page.getByText('Added to the integration queue').first()).toBeVisible()

    await page.goto(`/repos/${repo}`)
    const queue = page.getByTestId('queue-panel')
    await expect(queue).toContainText('Extend the notes')
    await page.getByRole('button', { name: 'Run queue' }).click()
    await expect(page.getByTestId('proposals-panel')).toContainText('No open proposals')
    await expect(page.getByRole('link', { name: 'Extend the notes' }).first()).toBeVisible()
    const head = await alice.get(`/v1/repos/${repo}/log?rev=main&page_size=3`)
    expect(head.json.changesets[0].message).toContain('Notes')
  })

  test('fencing tokens are checked before they are sent, and a held lease blocks until presented', async ({
    page,
    cast,
  }) => {
    const { repo } = await freshRepo(cast, 'fence')
    const run = await startRun(cast, repo, 'refactor-bot')
    await setPolicy(
      cast.as('dana'),
      repo,
      policyDocument({ requireTests: false, approvals: { min: 0, min_for_agents: 0 } }),
    )
    const leased = await run.as('POST', `/v1/repos/${repo}/leases`, {
      scope: 'src/**',
      exclusive: true,
      ttl_ms: 600_000,
    })
    const token = leased.json.lease.fencing_token as number
    const mine = await commitFiles(
      run.api,
      repo,
      { 'src/app.ts': 'export const answer = 7\n' },
      'Seven',
      {
        runId: run.runId,
      },
    )
    const p = await openProposal(run.api, repo, {
      head: mine.head,
      title: 'Seven',
      runId: run.runId,
    })

    await open(page, repo, p.id)
    await expect(page.getByTestId('blockers')).toContainText('An exclusive lease on src/**')
    await page.getByTestId('act-integrate').click()
    const dialog = page.getByRole('dialog', { name: 'Integrate this proposal' })
    await expect(dialog).toContainText(`src/** (token ${token})`)
    await dialog.getByLabel('Fencing tokens (optional)').fill('seven')
    await expect(dialog.getByText(/numbers/i)).toBeVisible()
    await expect(dialog.getByRole('button', { name: 'Integrate', exact: true })).toBeDisabled()
    await dialog.getByLabel('Fencing tokens (optional)').fill(String(token))
    await dialog.getByRole('button', { name: 'Integrate', exact: true }).click()
    await expect(dialog.getByText('Integrated', { exact: true })).toBeVisible()
  })
})
