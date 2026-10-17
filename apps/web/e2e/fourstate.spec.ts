import { expect, test } from './fixtures.ts'
import type { Page } from '@playwright/test'
import { failChecksFor } from './support/state.ts'
import { verdicts, waitLive } from './support/ui.ts'
import {
  amend,
  commitFiles,
  freshRepo,
  openProposal,
  policyDocument,
  review,
  setPolicy,
  validated,
} from './support/world.ts'

const strip = (page: Page) => page.getByRole('list', { name: /four independent states/ })
const pill = (page: Page, key: string) => strip(page).locator(`[data-state="${key}"]`)

/** A fresh repository with a landed commit and one open proposal by alice. */
async function setup(cast: Parameters<typeof freshRepo>[0], title = 'Double the answer') {
  const { repo } = await freshRepo(cast, 'flow')
  const alice = cast.as('alice')
  const c = await commitFiles(
    alice,
    repo,
    { 'src/app.ts': 'export const answer = 42\n' },
    'Fix the answer',
  )
  const p = await openProposal(alice, repo, {
    head: c.head,
    title,
    description: 'The answer is 42.',
  })
  return { repo, proposal: p.id, workspace: c.workspace, alice }
}

test.describe('the four states of a proposal change independently', () => {
  test.use({ persona: 'carol' })

  test('policy, validation, approval and integration each flip their own badge, live', async ({
    page,
    cast,
    openAs,
  }) => {
    const { repo, proposal } = await setup(cast)
    const dana = cast.as('dana')

    await page.goto(`/repos/${repo}/proposals/${proposal}`)
    await waitLive(page)

    // No policy: nothing is required, and the change merges cleanly.
    await expect(pill(page, 'mergeable')).toHaveAttribute('data-tone', 'ok')
    await expect(pill(page, 'validated')).toHaveAttribute('data-tone', 'na')
    await expect(pill(page, 'validated')).toContainText('none required')
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'na')
    await expect(pill(page, 'integrated')).toHaveAttribute('data-tone', 'pending')
    await expect(page.getByText('Nothing blocks integration right now.')).toBeVisible()

    // An administrator sets a policy that needs the tests and one approval. Nothing is reloaded:
    // the open page hears about it and re-checks.
    await setPolicy(
      dana,
      repo,
      policyDocument({ approvals: { min: 1, min_for_agents: 1, forbid_self_approval: true } }),
    )
    await expect(pill(page, 'validated')).toHaveAttribute('data-tone', 'blocked')
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'blocked')
    await expect(pill(page, 'approved')).toContainText('0/1')
    // Mergeable is unaffected by policy: that is the point of keeping the states apart.
    await expect(pill(page, 'mergeable')).toHaveAttribute('data-tone', 'ok')
    const blockers = page.getByTestId('blockers')
    await expect(blockers).toContainText('Needs 1 more approval')
    await expect(blockers).toContainText('has not passed on this exact tree')

    // Integrating now is refused, in words, and the stream does not move.
    const headBefore = (await dana.get(`/v1/repos/${repo}/streams/head?stream=main`)).json.head
    await page.getByTestId('act-integrate').click()
    const integrate = page.getByRole('dialog', { name: 'Integrate this proposal' })
    await expect(integrate).toContainText('Needs 1 more approval')
    await integrate.getByRole('button', { name: 'Integrate', exact: true }).click()
    await expect(integrate.getByText('Blocked: the stream was not changed')).toBeVisible()
    await integrate.getByRole('button', { name: 'Cancel' }).click()
    expect((await dana.get(`/v1/repos/${repo}/streams/head?stream=main`)).json.head).toBe(
      headBefore,
    )

    // Validation: asked for from the UI, run by the (real) execution plane, answered with a signed attestation.
    // carol is an integrator and may not request validation; alice (a writer) does, in her own session.
    await expect(page.getByTestId('act-validate')).toBeDisabled()
    const alicePage = await openAs('alice')
    await alicePage.goto(`/repos/${repo}/proposals/${proposal}`)
    await alicePage.getByTestId('act-validate').click()
    await expect(pill(page, 'validated')).toHaveAttribute('data-tone', 'pending', {
      timeout: 15_000,
    })
    await expect(pill(page, 'validated')).toHaveAttribute('data-tone', 'ok', { timeout: 30_000 })
    await expect(pill(page, 'validated')).toContainText('1/1 passed')
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'blocked')

    // Approval: bob reviews in his own session through the dialog.
    const bob = await openAs('bob')
    await bob.goto(`/repos/${repo}/proposals/${proposal}`)
    await bob.getByTestId('act-review').click()
    const dialog = bob.getByRole('dialog', { name: 'Write a review' })
    await expect(dialog).toContainText('recorded against head')
    await dialog.getByLabel('Approve').check()
    await dialog.getByRole('textbox', { name: 'Comment' }).fill('Looks right to me.')
    await dialog.getByRole('button', { name: 'Submit review' }).click()
    await expect(dialog).toHaveCount(0)

    // carol's page flips Approved live, with no reload.
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'ok', { timeout: 15_000 })
    await expect(pill(page, 'approved')).toContainText('1 approval')
    expect(await verdicts(strip(page))).toEqual({
      mergeable: 'ok',
      validated: 'ok',
      approved: 'ok',
      integrated: 'pending',
    })
    await expect(page.getByText('Nothing blocks integration right now.')).toBeVisible()

    // Integrate through the dialog.
    await page.getByTestId('act-integrate').click()
    await integrate.getByRole('button', { name: 'Integrate', exact: true }).click()
    await expect(integrate.getByText('Integrated', { exact: true })).toBeVisible()
    await expect(integrate).toContainText(/version \d+, fast-forward/)
    await integrate.getByRole('button', { name: 'Close', exact: true }).click()

    await expect(pill(page, 'integrated')).toHaveAttribute('data-tone', 'ok')
    await expect(pill(page, 'integrated')).toContainText('yes')
    await expect(page.getByTestId('proposal-state')).toHaveText('integrated')
    await expect(page.getByTestId('act-integrate')).toHaveCount(0)
    const headAfter = (await dana.get(`/v1/repos/${repo}/streams/head?stream=main`)).json.head
    expect(headAfter).not.toBe(headBefore)
    // The reviewer's own page agrees, again without a reload.
    await expect(pill(bob, 'integrated')).toHaveAttribute('data-tone', 'ok', { timeout: 15_000 })
  })

  test('an approval stops counting when the author pushes again, and the page says so', async ({
    page,
    cast,
  }) => {
    const { repo, proposal, workspace, alice } = await setup(cast, 'Rename the helper')
    await setPolicy(
      cast.as('dana'),
      repo,
      policyDocument({ approvals: { min: 1, forbid_self_approval: true }, requireTests: false }),
    )
    await review(cast.as('bob'), repo, proposal, 'approve', 'Fine')

    await page.goto(`/repos/${repo}/proposals/${proposal}`)
    await waitLive(page)
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'ok')

    // alice pushes another commit and moves the proposal head.
    const next = await amend(
      alice,
      repo,
      workspace,
      { 'src/app.ts': 'export const answer = 43\n' },
      'Another thought',
    )
    await alice.ok_(
      alice.patch(`/v1/repos/${repo}/proposals/${proposal}`, { new_head: next.head }),
      'update',
    )

    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'blocked', {
      timeout: 15_000,
    })
    await expect(pill(page, 'approved')).toContainText('0/1')
    await page.getByRole('tab', { name: /Reviews/ }).click()
    const reviews = page.getByTestId('reviews-tab')
    await expect(reviews.getByText('Some approvals are stale')).toBeVisible()
    await expect(reviews.getByText('stale', { exact: true })).toBeVisible()
    await expect(reviews).toContainText('no longer count')
  })

  test('raising the bar in the policy flips Approved back, lowering it flips it to not required', async ({
    page,
    cast,
  }) => {
    const { repo, proposal } = await setup(cast, 'Tidy the notes')
    const dana = cast.as('dana')
    await setPolicy(dana, repo, policyDocument({ approvals: { min: 1 }, requireTests: false }))
    await review(cast.as('bob'), repo, proposal, 'approve', 'ok')

    await page.goto(`/repos/${repo}/proposals/${proposal}`)
    await waitLive(page)
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'ok')

    await setPolicy(dana, repo, policyDocument({ approvals: { min: 2 }, requireTests: false }))
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'blocked', {
      timeout: 15_000,
    })
    await expect(pill(page, 'approved')).toContainText('1/2')

    await setPolicy(dana, repo, policyDocument({ requireTests: false }))
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'ok', { timeout: 15_000 })
  })

  test('a failed check is a verdict of its own and cannot be argued away by an approval', async ({
    page,
    cast,
  }) => {
    const { repo, proposal } = await setup(cast, 'Break the build')
    const dana = cast.as('dana')
    failChecksFor(proposal)
    await setPolicy(dana, repo, policyDocument({ approvals: { min: 1 } }))
    await review(cast.as('bob'), repo, proposal, 'approve', 'ok')
    await validated(cast.as('alice'), repo, proposal)

    await page.goto(`/repos/${repo}/proposals/${proposal}`)
    await expect(pill(page, 'validated')).toHaveAttribute('data-tone', 'failed')
    await expect(pill(page, 'approved')).toHaveAttribute('data-tone', 'ok')
    await expect(pill(page, 'mergeable')).toHaveAttribute('data-tone', 'ok')
    await expect(page.getByTestId('blockers')).toContainText('failed on this exact tree')
    // Failure is a denial, not something an integrator can fix by retrying.
    await page.getByTestId('act-integrate').click()
    const dialog = page.getByRole('dialog', { name: 'Integrate this proposal' })
    await dialog.getByRole('button', { name: 'Integrate', exact: true }).click()
    await expect(dialog.getByText('Refused by policy: the stream was not changed')).toBeVisible()
  })
})
