import { expect, test } from './fixtures.ts'

// erin has the reader role: she may look at everything and change nothing. The server enforces that;
// the UI must not offer what will be refused, and must say why.
test.describe('a read-only user', () => {
  test.use({ persona: 'erin' })

  test('sees the live board but is offered no way to change anything', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    await expect(page.getByTestId('proposals-panel').locator('article[data-proposal]')).toHaveCount(
      6,
    )
    const run = page.getByRole('button', { name: 'Run queue' })
    await expect(run).toHaveAttribute('aria-disabled', 'true')
    await expect(run).toHaveAccessibleDescription(/integrator/)
  })

  test('has no Admin tab, and the admin page says what it needs instead of showing controls', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByRole('link', { name: 'Admin' })).toHaveCount(0)
    await page.goto(`/repos/${state.demo.repo}/admin`)
    await expect(page.getByText('Administration needs the admin role')).toBeVisible()
    await expect(page.getByRole('tab', { name: 'Policy' })).toHaveCount(0)
    await expect(page.getByTestId('policy-editor')).toHaveCount(0)
    await expect(page.getByTestId('mint-open')).toHaveCount(0)
  })

  test('on a proposal every action is disabled with the role it needs', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.docs}`)
    const expectations: [string, RegExp][] = [
      ['act-integrate', /integrator/],
      ['act-review', /reviewer/],
      ['act-validate', /writer/],
      ['act-enqueue', /writer|integrator/],
      ['act-close', /writer|admin/],
    ]
    for (const [id, role] of expectations) {
      const button = page.getByTestId(id)
      await expect(button, id).toHaveAttribute('aria-disabled', 'true')
      await expect(button, id).toHaveAccessibleDescription(role)
    }
    // Clicking a disabled action does nothing: no dialog opens.
    await page.getByTestId('act-integrate').click({ force: true })
    await expect(page.getByRole('dialog')).toHaveCount(0)
    // She can still read everything: the diff, the reviews, the evidence.
    await expect(page.getByTestId('changes-tab')).toContainText('docs/api/sessions.md')
    await page.getByRole('tab', { name: /Reviews/ }).click()
    await expect(
      page.getByTestId('reviews-tab').getByRole('button', { name: 'Write a review' }),
    ).toHaveAttribute('aria-disabled', 'true')
  })

  test('cannot create repositories, streams or tokens for others', async ({ page, state }) => {
    await page.goto('/')
    await expect(page.getByRole('button', { name: 'New repository' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    await page.goto(`/repos/${state.demo.repo}/streams`)
    await expect(page.getByRole('button', { name: 'New stream' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    await page.goto('/access')
    await page.getByTestId('mint-open').click()
    await expect(page.getByRole('dialog').getByLabel('Identity')).toBeDisabled()
  })

  test('the server, not the page, is the authority: a forced request is refused', async ({
    page,
    state,
    cast,
  }) => {
    const refused = await cast
      .as('erin')
      .post(`/v1/repos/${state.demo.repo}/proposals/${state.demo.proposals.docs}/reviews`, {
        decision: 'approve',
      })
    expect(refused.status).toBe(403)
    expect(refused.json.error.code).toBe('permission_denied')
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.docs}`)
    await page.getByRole('tab', { name: /Reviews/ }).click()
    // erin's attempt left no review behind
    await expect(page.getByTestId('reviews-tab')).not.toContainText('user:erin')
  })
})
