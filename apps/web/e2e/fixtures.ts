// The test fixtures shared by every spec: a page that is already signed in as a persona, the same cast
// the API seeding used, and a guard that fails any test whose pages log an error, throw, or violate
// the server's Content-Security-Policy.
import { expect, test as base } from '@playwright/test'
import type { Browser, Page } from '@playwright/test'
import { castOf, readState } from './support/state.ts'
import type { State } from './support/state.ts'
import type { Cast, Who } from './support/world.ts'

export const TOKEN_KEY = 'agentgit.token'
export { expect }

interface Fixtures {
  state: State
  cast: Cast
  /** Who the default `page` is signed in as. `null` leaves it signed out. */
  persona: Who | null
  /** Tolerate console errors matching this pattern (for tests that provoke 4xx on purpose). */
  allowConsole: (pattern: RegExp) => void
  /** A second, independent browser session signed in as someone else. */
  openAs: (who: Who | null, opts?: { colorScheme?: 'light' | 'dark' }) => Promise<Page>
}

type Problems = { list: string[]; allowed: RegExp[] }

function instrument(page: Page, problems: Problems): void {
  page.on('console', (m) => {
    if (m.type() !== 'error' && m.type() !== 'warning') return
    const where = m.location().url ? ` [${m.location().url.replace(/^https?:\/\/[^/]+/, '')}]` : ''
    problems.list.push(`console.${m.type()}: ${m.text()}${where}`)
  })
  page.on('pageerror', (e) => problems.list.push(`uncaught exception: ${e.message}`))
  page.on('crash', () => problems.list.push('the page crashed'))
}

async function signInInit(page: Page, token: string | null): Promise<void> {
  if (!token) return
  await page.addInitScript(
    ([key, value]) => {
      try {
        if (!window.sessionStorage.getItem(key as string)) {
          window.sessionStorage.setItem(key as string, value as string)
        }
      } catch {
        /* storage blocked */
      }
    },
    [TOKEN_KEY, token],
  )
}

export const test = base.extend<Fixtures & { problems: Problems }>({
  baseURL: async ({}, use) => {
    await use(readState().baseURL)
  },
  state: async ({}, use) => {
    await use(readState())
  },
  cast: async ({ state }, use) => {
    await use(castOf(state))
  },
  persona: ['dana', { option: true }],

  problems: [
    async ({}, use, testInfo) => {
      // With the server's limiter switched on (AG_E2E_RPS) the browser itself logs every 429 as an error; the
      // client is expected to wait those out, so they are not problems then.
      const problems: Problems = {
        list: [],
        allowed: process.env.AG_E2E_RPS ? [/status of 429/] : [],
      }
      await use(problems)
      const unexpected = problems.list.filter((p) => !problems.allowed.some((re) => re.test(p)))
      if (unexpected.length) {
        await testInfo.attach('console-problems', { body: unexpected.join('\n') })
      }
      expect(unexpected, 'no console errors, warnings or uncaught exceptions').toEqual([])
    },
    { auto: true },
  ],

  allowConsole: async ({ problems }, use) => {
    await use((re) => void problems.allowed.push(re))
  },

  page: async ({ page, persona, cast, problems }, use) => {
    instrument(page, problems)
    await signInInit(page, persona ? cast.token(persona) : null)
    await use(page)
  },

  openAs: async ({ browser, cast, problems, baseURL }, use) => {
    const contexts: { close(): Promise<void> }[] = []
    await use(async (who, opts = {}) => {
      const ctx = await (browser as Browser).newContext({
        baseURL,
        viewport: { width: 1440, height: 900 },
        locale: 'en-US',
        timezoneId: 'UTC',
        colorScheme: opts.colorScheme ?? 'light',
        reducedMotion: 'reduce',
      })
      contexts.push(ctx)
      const p = await ctx.newPage()
      instrument(p, problems)
      await signInInit(p, who ? cast.token(who) : null)
      return p
    })
    await Promise.all(contexts.map((c) => c.close()))
  },
})
