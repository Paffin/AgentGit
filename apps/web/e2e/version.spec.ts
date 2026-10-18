import type { Browser } from '@playwright/test'
import { cpSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { TOKEN_KEY, expect, test } from './fixtures.ts'
import { startServer } from './support/server.ts'
import type { RunningServer } from './support/server.ts'
import { expectAccessible } from './support/ui.ts'

// Does the page belong to the server? Both versions are read from the real thing: the UI's from the build it came
// from (`/ui-version.json`, written by the build), the server's from `GET /v1/capabilities`. To get a REAL
// mismatch, a second real server serves a copy of the build whose `ui-version.json` names another version.

const DIST = resolve(import.meta.dirname, '../dist')

test('a UI that belongs to its server says nothing, and the build names the same version as the server', async ({
  page,
  state,
  request,
}) => {
  const ui = await (await request.get(`${state.baseURL}/ui-version.json`)).json()
  const caps = await (await request.get(`${state.baseURL}/v1/capabilities`)).json()
  expect(ui.version).toBe(caps.server_version)
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
  // The check has run (it is the first thing the shell does) and found nothing to say.
  await expect(page.getByTestId('version-banner')).toHaveCount(0)
})

test.describe('a UI build of another version', () => {
  let server: RunningServer
  let copy: string
  let serverVersion = ''

  test.beforeAll(async () => {
    copy = mkdtempSync(join(tmpdir(), 'agentgit-ui-'))
    cpSync(DIST, copy, { recursive: true })
    server = await startServer({ ui: copy })
    serverVersion = (await (await fetch(`${server.baseURL}/v1/capabilities`)).json()).server_version
  })
  test.afterAll(async () => {
    await server?.stop()
    rmSync(copy, { recursive: true, force: true })
  })

  async function open(browser: Browser, uiVersion: string) {
    writeFileSync(join(copy, 'ui-version.json'), JSON.stringify({ version: uiVersion }))
    rmSync(join(copy, 'ui-version.json.br'), { force: true })
    rmSync(join(copy, 'ui-version.json.gz'), { force: true })
    const ctx = await browser.newContext({ baseURL: server.baseURL, colorScheme: 'light' })
    const page = await ctx.newPage()
    await page.addInitScript(
      ([k, v]) => sessionStorage.setItem(k as string, v as string),
      [TOKEN_KEY, server.adminToken],
    )
    await page.goto('/')
    return { ctx, page }
  }

  test('warns that the server was updated after the page loaded, and offers a reload', async ({
    browser,
  }, testInfo) => {
    const { ctx, page } = await open(browser, '0.0.1')
    const banner = page.getByTestId('version-banner')
    await expect(banner).toBeVisible()
    await expect(banner).toContainText('This page and the server are different versions')
    await expect(banner).toContainText(
      `This page is version 0.0.1; the server runs ${serverVersion}`,
    )
    await expect(banner).toContainText('The server was updated after this page loaded')
    await expect(page.getByTestId('version-reload')).toBeVisible()
    await expectAccessible(page, testInfo, 'version banner')
    await ctx.close()
  })

  test('warns that the page is newer than the server', async ({ browser }) => {
    const { ctx, page } = await open(browser, '99.0.0')
    await expect(page.getByTestId('version-banner')).toContainText(
      'The server is older than the page',
    )
    await ctx.close()
  })

  test('a patch-level difference is compatible and says nothing', async ({ browser }) => {
    const [major, minor] = serverVersion.split('.')
    const { ctx, page } = await open(browser, `${major}.${minor}.999`)
    await expect(page.getByRole('heading', { name: 'Repositories', level: 1 })).toBeVisible()
    await expect(page.getByTestId('version-banner')).toHaveCount(0)
    await ctx.close()
  })
})
