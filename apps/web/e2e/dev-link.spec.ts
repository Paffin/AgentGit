// The sign-in link printed by `ag-server dev`: a REAL `ag-server dev` process (its own data directory and port), a
// real browser. The credential travels in the URL fragment, which the browser never sends to the server; the UI
// takes it out of the address bar at once, signs in without the "Continue" step, and keeps it like a pasted token.
import { spawn } from 'node:child_process'
import type { ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, test, TOKEN_KEY } from './fixtures.ts'
import { serverBinary } from './support/server.ts'

interface Dev {
  base: string
  link: string
  stop(): Promise<void>
}

/** Starts `ag-server dev` on a free port and reads the link from its banner. */
async function startDev(dataDir: string): Promise<Dev> {
  const child: ChildProcess = spawn(serverBinary(), ['dev', '--data-dir', dataDir, '--port', '0'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, NO_COLOR: '1' },
  })
  let out = ''
  child.stdout?.on('data', (d: Buffer) => (out += d.toString()))
  child.stderr?.on('data', (d: Buffer) => (out += d.toString()))
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`ag-server dev exited early:\n${out}`)
    if (out.includes('Credential expires')) break
    await new Promise((r) => setTimeout(r, 50))
  }
  const link = /(http:\/\/127\.0\.0\.1:\d+\/repos\/demo\/welcome#token=\S+)/.exec(out)?.[1]
  if (!link) throw new Error(`no sign-in link in the banner (the server binary must embed the Web UI: scripts/build_ui.sh, then cargo build):\n${out}`)
  return {
    base: new URL(link).origin,
    link,
    stop: () =>
      new Promise((res) => {
        child.once('exit', () => res())
        child.kill('SIGINT')
      }),
  }
}

test.describe('the sign-in link of ag-server dev', () => {
  test.use({ persona: null })

  test('signs in, lands where the link points, and leaves no credential in the address bar', async ({
    page,
  }) => {
    const dir = mkdtempSync(join(tmpdir(), 'ag-dev-e2e-'))
    const dev = await startDev(join(dir, 'data'))
    try {
      const requests: string[] = []
      page.on('request', (r) => requests.push(r.url()))
      await page.goto(dev.link)

      await expect(page).toHaveURL(/\/repos\/demo\/welcome$/)
      await expect(page.getByRole('heading', { name: /demo\/welcome/ }).first()).toBeVisible()
      // Signed in as the dev administrator, with no "Continue" step in between.
      expect(await page.evaluate(() => location.hash)).toBe('')
      expect(page.url()).not.toContain('agt_')
      const stored = await page.evaluate((k) => sessionStorage.getItem(k), TOKEN_KEY)
      expect(stored).toMatch(/^agt_/)
      expect(await page.evaluate(() => localStorage.length)).toBe(0)
      // The credential went to the server only in an Authorization header, never in a URL.
      const token = new URL(dev.link).hash.replace('#token=', '')
      expect(requests.filter((u) => u.includes(token))).toEqual([])
      // And the session survives a reload.
      await page.reload()
      await expect(page).toHaveURL(/\/repos\/demo\/welcome$/)
      await expect(page.getByRole('heading', { name: /demo\/welcome/ }).first()).toBeVisible()
    } finally {
      await page.goto('about:blank') // leave before the server goes: no live connection to complain
      await dev.stop()
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test('the demo repository is there, with its sample files', async ({ page }) => {
    const dir = mkdtempSync(join(tmpdir(), 'ag-dev-e2e-'))
    const dev = await startDev(join(dir, 'data'))
    try {
      await page.goto(dev.link)
      // Let the page finish loading before leaving it (a dynamic import cut off mid-way is logged by Firefox).
      await expect(page.getByRole('heading', { name: /demo\/welcome/ }).first()).toBeVisible()
      const r = await page.request.get(`${dev.base}/v1/repos/demo/welcome/tree/list?rev=main`, {
        headers: { Authorization: `Bearer ${new URL(dev.link).hash.replace('#token=', '')}` },
      })
      expect(r.status()).toBe(200)
      const names = JSON.stringify(await r.json())
      for (const f of ['README.md', 'SECURITY.md', 'docs', 'src']) expect(names).toContain(f)
    } finally {
      await page.goto('about:blank') // leave before the server goes: no live connection to complain
      await dev.stop()
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test('after a restart the old link says why it no longer works, and the new one signs in', async ({
    page,
    allowConsole,
  }) => {
    allowConsole(/status of 401/)
    const dir = mkdtempSync(join(tmpdir(), 'ag-dev-e2e-'))
    const data = join(dir, 'data')
    const first = await startDev(data)
    await first.stop()
    const second = await startDev(data)
    try {
      // Same data directory, same demo repository, a new credential: the old link is refused.
      await page.goto(first.link.replace(first.base, second.base))
      await expect(page).toHaveURL(/\/signin/)
      await expect(page.getByRole('alert').filter({ hasText: 'did not work' })).toContainText(
        'restarted',
      )
      expect(page.url()).not.toContain('agt_')
      expect(await page.evaluate((k) => sessionStorage.getItem(k), TOKEN_KEY)).toBeNull()

      await page.goto(second.link)
      await expect(page).toHaveURL(/\/repos\/demo\/welcome$/)
    } finally {
      await page.goto('about:blank')
      await second.stop()
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
