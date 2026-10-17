import { expect, test } from './fixtures.ts'
import type { Page } from '@playwright/test'
import { HOSTILE, HOSTILE_REPO } from './support/seed.ts'

// The UI shows text it does not control: titles, descriptions, diffs, file names and contents, review
// bodies, run summaries. None of it may ever run or turn into markup.

// No inline script and no eval. Styles may be inline: WebKit refused the app's CSSOM-driven styles under a stricter
// style-src (found by the WebKit CI job), so style-src carries 'unsafe-inline' like it always did.
const CSP =
  "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'"

test.describe('content security policy', () => {
  test('the server sends a strict CSP with the UI, and the built page needs nothing more', async ({
    request,
    baseURL,
  }) => {
    const res = await request.get(`${baseURL}/`)
    expect(res.status()).toBe(200)
    const csp = res.headers()['content-security-policy']
    expect(csp).toBeTruthy()
    for (const directive of CSP.split('; ')) expect(csp).toContain(directive)
    expect(csp).not.toContain('unsafe-eval')
    expect(csp).not.toMatch(/script-src[^;]*unsafe-inline/)
    const html = await res.text()
    expect(html).not.toMatch(/<script(?![^>]*\bsrc=)[^>]*>\s*\S/)
    expect(html).not.toMatch(/https?:\/\/(?!127\.0\.0\.1|localhost)/)
  })

  test('every main screen loads and works under it: no violations, nothing from another origin', async ({
    page,
    state,
  }) => {
    const external: string[] = []
    page.on('request', (r) => {
      const u = new URL(r.url())
      if (u.protocol.startsWith('http') && u.origin !== new URL(state.baseURL).origin)
        external.push(r.url())
    })
    await page.addInitScript(() => {
      ;(window as unknown as { __csp: string[] }).__csp = []
      document.addEventListener('securitypolicyviolation', (e) =>
        (window as unknown as { __csp: string[] }).__csp.push(
          `${e.violatedDirective} ${e.blockedURI}`,
        ),
      )
    })
    for (const path of [
      '/',
      `/repos/${state.demo.repo}`,
      `/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`,
      `/repos/${state.demo.repo}/files?path=src/server.ts`,
      `/repos/${state.demo.repo}/admin`,
    ]) {
      await page.goto(path)
      await expect(page.locator('main')).toBeVisible()
      await page.waitForTimeout(400)
      expect(
        await page.evaluate(() => (window as unknown as { __csp: string[] }).__csp),
        path,
      ).toEqual([])
    }
    expect(external).toEqual([])
  })

  test('the app uses neither eval nor inline handlers', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}`)
    await expect(page.getByTestId('board-summary')).toBeVisible()
    expect(
      await page.evaluate(
        () => document.querySelectorAll('[onclick],[onerror],[onload],[onmouseover]').length,
      ),
    ).toBe(0)
    expect(await page.evaluate(() => document.querySelectorAll('script:not([src])').length)).toBe(0)
  })
})

async function noMarkupFromServerText(page: Page) {
  // Nothing the server wrote has become an element, an attribute or a running script.
  expect(
    await page.evaluate(() => (window as unknown as { __pwned?: string }).__pwned),
  ).toBeUndefined()
  expect(
    await page.evaluate(
      () =>
        document.querySelectorAll(
          'main img[src="x"], main svg[onload], main iframe, main style, [onerror], [onload]',
        ).length,
    ),
  ).toBe(0)
  expect(
    await page.evaluate(() => document.querySelectorAll('main a[href^="javascript:" i]').length),
  ).toBe(0)
}

test.describe('hostile text from the server', () => {
  test.beforeEach(async ({ page }) => {
    // Any alert/confirm/prompt would mean script ran.
    page.on('dialog', async (d) => {
      await d.dismiss()
      throw new Error(`a script opened a ${d.type()} dialog: ${d.message()}`)
    })
  })

  test('the repository list shows a hostile description as text', async ({ page }) => {
    await page.goto('/')
    const row = page.getByRole('row').filter({ hasText: HOSTILE_REPO })
    await expect(row).toContainText(HOSTILE.description)
    await noMarkupFromServerText(page)
  })

  test('board, proposal, reviews, diff and timeline show it verbatim and inert', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${HOSTILE_REPO}`)
    await expect(page.getByTestId('proposals-panel')).toContainText(HOSTILE.proposalTitle)
    await expect(page.getByTestId('task-graph')).toContainText('<iframe')
    await noMarkupFromServerText(page)

    await page.goto(`/repos/${HOSTILE_REPO}/proposals/${state.hostile.proposal}`)
    await expect(page.getByTestId('proposal-title')).toHaveText(HOSTILE.proposalTitle)
    await expect(page.locator('main')).toContainText(HOSTILE.proposalBody.split('\n')[0]!)
    // The link-looking text in the description is just text.
    await expect(page.locator('main').getByRole('link', { name: 'click' })).toHaveCount(0)
    const diff = page.getByTestId('changes-tab')
    await expect(diff).toContainText("window.__pwned='file'")
    await expect(diff).toContainText(HOSTILE.file)
    await noMarkupFromServerText(page)

    await page.getByRole('tab', { name: /Reviews/ }).click()
    await expect(page.getByTestId('reviews-tab')).toContainText(HOSTILE.review)
    await page.getByRole('tab', { name: /Timeline/ }).click()
    await noMarkupFromServerText(page)
  })

  test('file contents, file names, history and run summaries are text too', async ({ page }) => {
    await page.goto(`/repos/${HOSTILE_REPO}/files`)
    await expect(page.getByRole('table', { name: 'Directory listing' })).toContainText('index.html')
    await page.goto(`/repos/${HOSTILE_REPO}/files?path=index.html`)
    await expect(page.getByTestId('file-viewer')).toContainText("window.__pwned='file'")
    await page.getByTestId('file-viewer').getByLabel('Blame').check()
    await expect(page.getByTestId('blame')).toContainText("window.__pwned='file'")
    await noMarkupFromServerText(page)

    await page.goto(`/repos/${HOSTILE_REPO}/history`)
    await expect(page.getByTestId('history-log')).toContainText('<u>underlined</u>')
    await noMarkupFromServerText(page)

    await page.goto(`/repos/${HOSTILE_REPO}/agents`)
    await page.getByRole('radio', { name: 'History' }).check()
    await expect(page.locator('main')).toContainText(HOSTILE.runSummary)
    await noMarkupFromServerText(page)

    await page.goto(`/repos/${HOSTILE_REPO}/admin`)
    await page.getByRole('tab', { name: 'Audit log' }).click()
    await expect(page.getByRole('table').last()).toContainText('proposal.create')
    await noMarkupFromServerText(page)
  })

  test('an agent’s provenance rationale is untrusted text, never markup', async ({
    page,
    state,
    cast,
  }) => {
    // The demo's refactor-bot rationale is plain; add a hostile one in the hostile repo through the API.
    const { startRun, commitFiles, openProposal } = await import('./support/world.ts')
    const { signProvenance } = await import('./support/signing.ts')
    const { agentKey } = await import('./support/world.ts')
    const run = await startRun(cast, HOSTILE_REPO, 'refactor-bot')
    const c = await commitFiles(run.api, HOSTILE_REPO, { 'notes.txt': 'x\n' }, 'Notes', {
      runId: run.runId,
    })
    const p = await openProposal(run.api, HOSTILE_REPO, {
      head: c.head,
      title: 'Agent work',
      runId: run.runId,
    })
    const signed = signProvenance(agentKey('refactor-bot'), {
      changeset: c.head,
      agent: `agent:refactor-bot#${run.runId}`,
      runId: run.runId,
      model: '<b>model</b>',
      rationale: HOSTILE.proposalBody,
      createdAtMs: Date.now(),
    })
    const sent = await run.as('POST', `/v1/repos/${HOSTILE_REPO}/provenance`, signed)
    expect(sent.ok).toBe(true)
    void state
    await page.goto(`/repos/${HOSTILE_REPO}/proposals/${p.id}`)
    await page.getByRole('tab', { name: /Provenance/ }).click()
    await expect(page.getByTestId('provenance-tab')).toContainText(
      HOSTILE.proposalBody.split('\n')[0]!,
    )
    await expect(page.getByTestId('provenance-tab')).toContainText('<b>model</b>')
    await noMarkupFromServerText(page)
  })

  test('hostile text typed into dialogs and fields is sent as text and comes back as text', async ({
    page,
    state,
    openAs,
  }) => {
    const bob = await openAs('bob')
    await bob.goto(`/repos/${HOSTILE_REPO}/proposals/${state.hostile.proposal}`)
    await bob.getByTestId('act-review').click()
    const dialog = bob.getByRole('dialog', { name: 'Write a review' })
    await dialog
      .getByRole('textbox', { name: 'Comment' })
      .fill('<script>window.__pwned="typed"</script>')
    await dialog.getByRole('button', { name: 'Submit review' }).click()
    await expect(dialog).toHaveCount(0)
    await bob.getByRole('tab', { name: /Reviews/ }).click()
    await expect(bob.getByTestId('reviews-tab')).toContainText(
      '<script>window.__pwned="typed"</script>',
    )
    await noMarkupFromServerText(bob)
    void page
  })
})
