// The main screens of the app, each with how to get there and how to tell it has finished loading. Shared by the
// accessibility audit and the responsive layout checks, so both cover the same ground.
import type { Page } from '@playwright/test'
import { expect } from '../fixtures.ts'
import { waitLive } from './ui.ts'
import { EMPTY_REPO } from './seed.ts'
import type { State } from './state.ts'

/** A desktop-sized window: every column of a table is shown. */
const wide = (p: Page) => (p.viewportSize()?.width ?? 1440) >= 1000

export interface Screen {
  name: string
  path: (s: State) => string
  ready: (page: Page) => Promise<void>
  persona?: 'dana' | 'erin'
}

export const screens: Screen[] = [
  {
    name: 'repositories',
    path: () => '/',
    ready: async (p) => {
      await expect(p.getByRole('link', { name: 'acme/webapp' })).toBeVisible()
      // The counts arrive after the list; on a narrow screen their columns are not shown at all.
      if (wide(p)) {
        await expect(
          p.getByRole('row').filter({ hasText: 'acme/webapp' }).getByRole('cell').nth(3),
        ).not.toHaveText('–')
      }
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
    ready: async (p) =>
      expect(p.getByRole('table', { name: 'Reflog' })).toContainText(wide(p) ? 'carol' : '→'),
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
    name: 'connect your first agent (empty repository)',
    path: () => `/repos/${EMPTY_REPO}`,
    ready: async (p) => expect(p.getByTestId('connect-cmd-work')).toBeVisible(),
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
