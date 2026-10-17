import type { Page } from '@playwright/test'
import { expect, test } from './fixtures.ts'
import { expectAccessible, waitLive } from './support/ui.ts'
import type { State } from './support/state.ts'

// An automated accessibility audit (axe-core, WCAG 2.x A and AA plus best practices) of every main screen,
// in both themes. Serious and critical findings fail the test. Automated checks find only part of the
// problems: keyboard use and focus handling are tested separately (keyboard.spec.ts).

interface Screen {
  name: string
  path: (s: State) => string
  ready: (page: Page) => Promise<void>
  persona?: 'dana' | 'erin'
}

const screens: Screen[] = [
  {
    name: 'repositories',
    path: () => '/',
    ready: async (p) => {
      await expect(p.getByRole('link', { name: 'acme/webapp' })).toBeVisible()
      await expect(
        p.getByRole('row').filter({ hasText: 'acme/webapp' }).getByRole('cell').nth(3),
      ).not.toHaveText('–')
    },
  },
  {
    name: 'live board',
    path: (s) => `/repos/${s.demo.repo}`,
    ready: async (p) => {
      await waitLive(p)
      await expect(p.getByTestId('proposals-panel').locator('article[data-proposal]')).toHaveCount(
        6,
      )
      await expect(p.getByTestId('proposals-panel').getByText('checking')).toHaveCount(0)
      await expect(p.getByTestId('task-graph').locator('[data-task]')).toHaveCount(8)
    },
  },
  {
    name: 'proposals list',
    path: (s) => `/repos/${s.demo.repo}/proposals`,
    ready: async (p) =>
      expect(p.getByRole('heading', { name: 'Proposals', level: 1 })).toBeVisible(),
  },
  {
    name: 'proposal detail (changes)',
    path: (s) => `/repos/${s.demo.repo}/proposals/${s.demo.proposals.extract}`,
    ready: async (p) => {
      await expect(p.getByTestId('changes-tab')).toContainText('3 files changed')
      await expect(
        p.getByRole('list', { name: /four independent states/ }).getByText('checking'),
      ).toHaveCount(0)
    },
  },
  {
    name: 'proposal detail (conflict)',
    path: (s) => `/repos/${s.demo.repo}/proposals/${s.demo.proposals.formal}`,
    ready: async (p) => expect(p.getByTestId('conflicts')).toBeVisible(),
  },
  {
    name: 'streams',
    path: (s) => `/repos/${s.demo.repo}/streams`,
    ready: async (p) => expect(p.getByRole('table', { name: 'Reflog' })).toContainText('carol'),
  },
  {
    name: 'history',
    path: (s) => `/repos/${s.demo.repo}/history`,
    ready: async (p) =>
      expect(p.getByTestId('history-log')).toContainText('Import the webapp skeleton'),
  },
  {
    name: 'files',
    path: (s) => `/repos/${s.demo.repo}/files`,
    ready: async (p) =>
      expect(p.getByRole('table', { name: 'Directory listing' })).toContainText('README.md'),
  },
  {
    name: 'file with blame',
    path: (s) => `/repos/${s.demo.repo}/files?path=README.md`,
    ready: async (p) => {
      await p.getByTestId('file-viewer').getByLabel('Blame').check()
      await expect(p.getByTestId('blame').getByRole('table')).toBeVisible()
    },
  },
  {
    name: 'compare',
    path: (s) => `/repos/${s.demo.repo}/compare?from=main~3&to=main`,
    ready: async (p) => expect(p.getByRole('heading', { name: 'Compare revisions' })).toBeVisible(),
  },
  {
    name: 'agents (runs)',
    path: (s) => `/repos/${s.demo.repo}/agents`,
    ready: async (p) => expect(p.getByRole('table').first()).toContainText('refactor-bot'),
  },
  {
    name: 'admin (policy)',
    path: (s) => `/repos/${s.demo.repo}/admin`,
    ready: async (p) => {
      await expect(p.getByTestId('policy-text')).toHaveValue(/protected_paths/)
      await expect(p.getByTestId('policy-diff')).toBeVisible()
    },
  },
  {
    name: 'admin (credentials)',
    path: (s) => `/repos/${s.demo.repo}/admin`,
    ready: async (p) => {
      await p.getByRole('tab', { name: 'Credentials' }).click()
      await expect(p.getByRole('table', { name: 'Credentials' })).toContainText('dana')
    },
  },
  {
    name: 'admin (audit log)',
    path: (s) => `/repos/${s.demo.repo}/admin`,
    ready: async (p) => {
      await p.getByRole('tab', { name: 'Audit log' }).click()
      await expect(p.getByRole('table').last()).toContainText('proposal.create')
    },
  },
  {
    name: 'credentials and roles',
    path: () => '/access',
    ready: async (p) => expect(p.getByTestId('mint-open')).toBeVisible(),
  },
  {
    name: 'read-only view of a proposal',
    persona: 'erin',
    path: (s) => `/repos/${s.demo.repo}/proposals/${s.demo.proposals.docs}`,
    ready: async (p) =>
      expect(p.getByTestId('act-integrate')).toHaveAttribute('aria-disabled', 'true'),
  },
]

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`axe, ${scheme} theme`, () => {
    test.use({ colorScheme: scheme })

    for (const screen of screens) {
      test(`${screen.name} has no serious or critical violations`, async ({
        page,
        state,
        openAs,
      }, testInfo) => {
        const target = screen.persona ? await openAs(screen.persona, { colorScheme: scheme }) : page
        await target.goto(screen.path(state))
        await screen.ready(target)
        // The follow-the-system theme really followed the system.
        const dark = await target.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches)
        expect(dark).toBe(scheme === 'dark')
        await expectAccessible(target, testInfo, `${screen.name} (${scheme})`)
      })
    }
  })
}

test.describe('axe, signed-out and error screens', () => {
  test.use({ persona: null })

  test('sign in', async ({ page }, testInfo) => {
    await page.goto('/signin')
    await expect(page.getByLabel('Access token')).toBeVisible()
    await expectAccessible(page, testInfo, 'sign in')
  })

  test('sign in with an error showing', async ({ page, allowConsole }, testInfo) => {
    allowConsole(/status of 401/)
    await page.goto('/signin')
    await page.getByLabel('Access token').fill('agt_bad_0000')
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await expect(page.getByRole('alert').filter({ hasText: 'Could not sign in' })).toBeVisible()
    await expectAccessible(page, testInfo, 'sign in error')
  })

  test('not found', async ({ page }, testInfo) => {
    await page.goto('/nowhere/at/all')
    await expect(page.getByText('This page does not exist')).toBeVisible()
    await expectAccessible(page, testInfo, 'not found')
  })

  test('offline', async ({ page }, testInfo) => {
    await page.goto('/offline')
    await expect(page.getByText('Cannot reach the server')).toBeVisible()
    await expectAccessible(page, testInfo, 'offline')
  })
})

test.describe('axe, dialogs', () => {
  test('the review, integrate and mint dialogs', async ({ page, state }, testInfo) => {
    await page.goto(`/repos/${state.demo.repo}/proposals/${state.demo.proposals.extract}`)
    await page.getByTestId('act-review').click()
    await expect(page.getByRole('dialog', { name: 'Write a review' })).toBeVisible()
    await expectAccessible(page, testInfo, 'review dialog')
    await page.keyboard.press('Escape')
    await page.getByTestId('act-integrate').click()
    await expect(page.getByRole('dialog', { name: 'Integrate this proposal' })).toBeVisible()
    await expectAccessible(page, testInfo, 'integrate dialog')
    await page.keyboard.press('Escape')
    await page.goto('/access')
    await page.getByTestId('mint-open').click()
    await expect(page.getByRole('dialog', { name: 'Mint a token' })).toBeVisible()
    await expectAccessible(page, testInfo, 'mint dialog')
  })
})
