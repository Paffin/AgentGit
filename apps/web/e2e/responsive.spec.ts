import type { Page } from '@playwright/test'
import { expect, test } from './fixtures.ts'
import { screens } from './support/screens.ts'
import { fitsViewport, measureOverflow, measureTargets, smallTargets } from './support/layout.ts'

// Layout at the viewports of the `tablet` (768 px) and `phone` (390 px, touch) projects, measured in the page:
// nothing makes the page scroll sideways, the navigation is reachable, tables either fit or scroll on purpose
// (as a labelled region the keyboard can reach), dialogs fit the window and keep their actions in reach, and the
// controls a finger has to hit are at least 40 px.
//
// These run only in those projects (see playwright.config.ts).

const MIN_TARGET = 40

test.describe('no screen scrolls the page sideways', () => {
  for (const screen of screens) {
    test(screen.name, async ({ page, state, openAs }, testInfo) => {
      const target = screen.persona ? await openAs(screen.persona) : page
      await target.goto(screen.path(state))
      await screen.ready(target)
      const o = await measureOverflow(target)
      await testInfo.attach('overflow', { body: JSON.stringify(o, null, 2) })
      expect(
        o.scrollWidth,
        `the page is ${o.scrollWidth}px wide in a ${o.width}px window; sticking out: ${o.offenders.join(' | ')}`,
      ).toBeLessThanOrEqual(o.width)
      expect(o.offenders, 'nothing sticks out of the window').toEqual([])
    })
  }

  test('the sign-in page and the error pages', async ({ page, state, openAs, allowConsole }) => {
    allowConsole(/status of 404/)
    const signedOut = await openAs(null)
    for (const path of ['/signin', '/offline', '/unauthorized?next=/access', '/nowhere/at/all']) {
      await signedOut.goto(path)
      await expect(signedOut.locator('main')).toBeVisible()
      const o = await measureOverflow(signedOut)
      expect(o.scrollWidth, path).toBeLessThanOrEqual(o.width)
      expect(o.offenders, path).toEqual([])
    }
    await page.goto(`/repos/${state.demo.repo}/proposals/prop_nonexistent`)
    await expect(page.getByText(/not found|does not exist/i).first()).toBeVisible()
    const o = await measureOverflow(page)
    expect(o.scrollWidth).toBeLessThanOrEqual(o.width)
  })
})

test.describe('navigation', () => {
  test('the header is whole and the account menu opens inside the window', async ({ page }) => {
    await page.goto('/repos')
    await expect(page.getByTestId('user-menu')).toBeVisible()
    for (const l of [
      page.getByRole('link', { name: /AgentGit/ }).first(),
      page.getByTestId('theme-toggle'),
      page.getByTestId('user-menu'),
    ]) {
      expect(await fitsViewport(page, l)).toBe(true)
    }
    await page.getByTestId('user-menu').click()
    const menu = page.getByRole('menu')
    await expect(menu).toBeVisible()
    expect(await fitsViewport(page, menu)).toBe(true)
    for (const item of await menu.getByRole('menuitem').all()) {
      expect((await item.boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(MIN_TARGET)
    }
    await page.keyboard.press('Escape')
    await expect(menu).toHaveCount(0)
  })

  test('every tab of a repository can be reached, and the current one is in view', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/history`)
    const tabs = page.getByRole('navigation', { name: 'Repository' })
    await expect(tabs.getByRole('link')).toHaveCount(5)
    // The current tab is scrolled into view on arrival, not left off-screen in a strip with no scrollbar.
    expect(await fitsViewport(page, tabs.getByRole('link', { name: 'Code' }))).toBe(true)
    for (const link of await tabs.getByRole('link').all()) {
      expect(await fitsViewport(page, link), `${await link.textContent()} is reachable`).toBe(true)
    }
    // ... and following one works by touch or click.
    await tabs.getByRole('link', { name: 'Agents & work' }).click()
    await expect(page).toHaveURL(/\/agents$/)
    expect(await fitsViewport(page, tabs.getByRole('link', { name: 'Agents & work' }))).toBe(true)
    await tabs.getByRole('link', { name: 'Overview' }).click()
    await expect(page).toHaveURL(new RegExp(`/repos/${state.demo.repo}$`))
  })

  test('the keyboard reaches a tab that starts out of view and brings it into view', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals`)
    const admin = page
      .getByRole('navigation', { name: 'Repository' })
      .getByRole('link', { name: 'Settings' })
    await admin.focus()
    expect(await fitsViewport(page, admin)).toBe(true)
  })
})

test.describe('tables', () => {
  test('a table that does not fit scrolls as a labelled region the keyboard can reach', async ({
    page,
    state,
  }) => {
    for (const path of [
      '/repos',
      `/repos/${state.demo.repo}/streams`,
      `/repos/${state.demo.repo}/files`,
      `/repos/${state.demo.repo}/agents`,
      '/access',
    ]) {
      await page.goto(path)
      await expect(page.getByRole('table').first()).toBeVisible()
      await page.waitForTimeout(500)
      const report = await page.evaluate(() =>
        Array.from(document.querySelectorAll('table')).map((t) => {
          const wrap = t.closest('[class*="wrap"], div') as HTMLElement | null
          // The nearest ancestor that scrolls sideways, if any.
          let scroller: HTMLElement | null = null
          for (let p = t.parentElement; p; p = p.parentElement) {
            if (
              /(auto|scroll)/.test(getComputedStyle(p).overflowX) &&
              p.scrollWidth > p.clientWidth + 1
            ) {
              scroller = p
              break
            }
          }
          return {
            name:
              t.querySelector('caption')?.textContent?.trim() ??
              t.getAttribute('aria-label') ??
              '?',
            scrolls: !!scroller,
            focusable: scroller?.tabIndex === 0,
            region:
              scroller?.getAttribute('role') === 'region' && !!scroller.getAttribute('aria-label'),
            wrapExists: !!wrap,
          }
        }),
      )
      for (const t of report) {
        if (t.scrolls) {
          expect(
            t.focusable && t.region,
            `${path}: table "${t.name}" scrolls but is not a labelled focusable region`,
          ).toBe(true)
        }
      }
    }
  })

  test('the wide diff and code views scroll inside themselves, not the page', async ({
    page,
    state,
  }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    await expect(page.getByTestId('changes-tab')).toContainText('3 files changed')
    const o = await measureOverflow(page)
    expect(o.scrollWidth).toBeLessThanOrEqual(o.width)
    await page.goto(`/repos/${state.demo.repo}/files?path=README.md`)
    await expect(page.getByTestId('file-viewer')).toBeVisible()
    const f = await measureOverflow(page)
    expect(f.scrollWidth).toBeLessThanOrEqual(f.width)
  })
})

test.describe('dialogs', () => {
  async function checkDialog(page: Page, name: string | RegExp, primary: string | RegExp) {
    const dialog = page.getByRole('dialog', { name })
    await expect(dialog).toBeVisible()
    const box = (await dialog.boundingBox())!
    const size = page.viewportSize()!
    expect(box.x, 'dialog left edge').toBeGreaterThanOrEqual(-1)
    expect(box.x + box.width, 'dialog right edge').toBeLessThanOrEqual(size.width + 1)
    expect(box.y, 'dialog top edge').toBeGreaterThanOrEqual(-1)
    expect(box.y + box.height, 'dialog bottom edge').toBeLessThanOrEqual(size.height + 1)
    // The action that matters is in reach without scrolling the page behind, and big enough for a finger.
    const action = dialog.getByRole('button', { name: primary })
    expect(await fitsViewport(page, action)).toBe(true)
    expect((await action.boundingBox())!.height).toBeGreaterThanOrEqual(MIN_TARGET)
    const closeSmall = await smallTargets(
      page,
      '[role="dialog"] button, [role="dialog"] input:not([type=hidden]), [role="dialog"] select, [role="dialog"] textarea',
      MIN_TARGET,
    )
    expect(closeSmall, 'controls inside the dialog').toEqual([])
    const o = await measureOverflow(page)
    expect(o.scrollWidth).toBeLessThanOrEqual(o.width)
    return dialog
  }

  test('review, integrate and close dialogs of a proposal', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    await expect(page.getByTestId('changes-tab')).toContainText('3 files changed')
    await page.getByTestId('act-review').click()
    await checkDialog(page, 'Write a review', /Submit|Send|Review/)
    await page.keyboard.press('Escape')
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await page.getByTestId('act-integrate').click()
    await checkDialog(page, 'Land this change request', /Land|Integrate/)
    await page.keyboard.press('Escape')
    await expect(page.getByRole('dialog')).toHaveCount(0)
  })

  test('mint a token', async ({ page }) => {
    await page.goto('/access')
    await page.getByTestId('mint-open').click()
    await checkDialog(page, 'Mint a token', /Mint/)
  })
})

test.describe('touch targets', () => {
  // Buttons, tabs, navigation and form fields: what a finger must hit. Links inside running text are exempt (WCAG 2.5.8).
  const CONTROLS =
    'button, [role="tab"], [role="menuitem"], .tab, .navlink, a.btn, input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"]), select, textarea, [role="combobox"], summary'

  for (const screen of screens.filter((s) =>
    [
      'repositories',
      'live board',
      'proposals list',
      'proposal detail (changes)',
      'streams',
      'history',
      'files',
      'compare',
      'agents (runs)',
      'admin (policy)',
      'credentials and roles',
    ].includes(s.name),
  )) {
    test(`${screen.name}: controls are at least ${MIN_TARGET}px`, async ({ page, state }) => {
      await page.goto(screen.path(state))
      await screen.ready(page)
      const small = await smallTargets(page, CONTROLS, MIN_TARGET)
      expect(small, 'controls smaller than a fingertip').toEqual([])
    })
  }

  test('checkboxes are hit through their labels', async ({ page, state }) => {
    await page.goto(`/repos/${state.demo.repo}/agents`)
    await expect(page.getByRole('table').first()).toBeVisible()
    const boxes = await measureTargets(page, 'input[type="checkbox"], input[type="radio"]')
    for (const b of boxes) expect(b.height, b.what).toBeGreaterThanOrEqual(MIN_TARGET)
  })
})
