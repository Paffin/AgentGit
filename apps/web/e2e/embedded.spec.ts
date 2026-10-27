import { expect, test } from '@playwright/test'
import { requireComponent } from './support/components.ts'
import { startServer } from './support/server.ts'
import type { RunningServer } from './support/server.ts'

// The single-binary story: `ag-server` with NO UI configuration serves the app out of its own binary (cargo
// feature `ui`, built after `scripts/build_ui.sh`). These tests start their own server for that, so they hold
// whatever way the main suite is run. A binary built without the UI makes them fail in CI (where it must have
// been built with it) and skip, with a marker (support/components.ts), on a laptop.

let server: RunningServer
let hasUi = false

test.beforeAll(async () => {
  server = await startServer({ ui: 'embedded' })
  const res = await fetch(`${server.baseURL}/`, { headers: { accept: 'text/html' } })
  hasUi = res.status === 200
  if (!hasUi && process.env.CI) {
    throw new Error(
      `ag-server answered ${res.status} for / without a UI configured: the binary was built without the UI. ` +
        'Run scripts/build_ui.sh before cargo build.',
    )
  }
})
test.afterAll(async () => server?.stop())
test.beforeEach(({}, info) =>
  requireComponent(
    info,
    'embedded-ui',
    hasUi ? null : 'this ag-server binary was built without the Web UI',
  ),
)

test.describe('the UI served from the binary', () => {
  test('the page, its assets and the headers that go with them', async ({ request }) => {
    const page = await request.get(`${server.baseURL}/`)
    expect(page.status()).toBe(200)
    const h = page.headers()
    expect(h['content-type']).toBe('text/html; charset=utf-8')
    expect(h['cache-control']).toBe('no-cache')
    expect(h['etag']).toBeTruthy()
    expect(h['content-security-policy']).toContain("script-src 'self'")
    expect(h['content-security-policy']).not.toMatch(/script-src[^;]*unsafe-inline/)
    expect(h['x-content-type-options']).toBe('nosniff')
    expect(h['content-encoding']).toBe('br')
    const html = await page.text()
    expect(html).toContain('<div id="app">')

    // Every script, stylesheet and module preload must be typed, compressed and cacheable forever.
    const script = /<script[^>]+src="([^"]+)"/.exec(html)?.[1]
    const style = /<link[^>]+rel="stylesheet"[^>]+href="([^"]+)"/.exec(html)?.[1]
    expect(script, 'a module script').toBeTruthy()
    expect(style, 'a stylesheet').toBeTruthy()
    const preloads = [...html.matchAll(/<link[^>]+rel="modulepreload"[^>]+href="([^"]+)"/g)].map(
      (match) => [match[1]!, 'text/javascript'] as const,
    )
    const assets = [
      [script ?? '', 'text/javascript'],
      [style ?? '', 'text/css'],
      ...preloads,
    ] as const
    await Promise.all(
      assets.map(async ([url, type]) => {
        const res = await request.get(`${server.baseURL}${url}`)
        expect(res.status(), url).toBe(200)
        expect(res.headers()['content-type'], url).toContain(type)
        expect(res.headers()['cache-control'], url).toBe('public, max-age=31536000, immutable')
        expect(res.headers()['content-encoding'], url).toBe('br')
      }),
    )

    // A conditional request is answered with 304.
    const again = await request.get(`${server.baseURL}/`, {
      headers: { 'if-none-match': h['etag'] ?? '' },
    })
    expect(again.status()).toBe(304)
  })

  test('app routes load the page; missing assets and the API keep their own answers', async ({
    request,
  }) => {
    for (const path of [
      '/signin',
      '/repos/acme/webapp/proposals/prop_1',
      '/auth/callback?code=x',
    ]) {
      const res = await request.get(`${server.baseURL}${path}`)
      expect(res.status(), path).toBe(200)
      expect(await res.text(), path).toContain('<div id="app">')
    }
    for (const path of ['/assets/missing-AAAAAAAA.js', '/v1/nothing', '/agwp.v1.Nothing/Call']) {
      const res = await request.get(`${server.baseURL}${path}`)
      expect(res.status(), path).toBe(404)
      expect((await res.json()).error.code, path).toBe('not_found')
    }
    expect((await request.get(`${server.baseURL}/healthz`)).status()).toBe(200)
    expect((await request.get(`${server.baseURL}/v1/capabilities`)).status()).toBe(200)
  })

  test('the app boots from the binary and a person can sign in with the bootstrap token', async ({
    page,
  }) => {
    const problems: string[] = []
    page.on('console', (m) => {
      if (m.type() === 'error' || m.type() === 'warning') problems.push(m.text())
    })
    page.on('pageerror', (e) => problems.push(e.message))
    await page.goto(`${server.baseURL}/repos/acme/webapp`)
    // Not signed in: the deep link leads to the sign-in page and remembers where to go.
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()
    await page.getByLabel('Access token').fill(server.adminToken)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await expect(page.getByTestId('user-menu')).toBeVisible()
    expect(problems, 'no console problems, no CSP violations').toEqual([])
  })
})

test.describe('the UI switched off', () => {
  test('ui.enabled = false leaves only the API', async () => {
    const api = await startServer({ ui: 'embedded', extraConfig: ['[ui]', 'enabled = false'] })
    try {
      const root = await fetch(`${api.baseURL}/`, { headers: { accept: 'text/html' } })
      expect(root.status).toBe(404)
      expect(((await root.json()) as { error: { code: string } }).error.code).toBe('not_found')
      expect((await fetch(`${api.baseURL}/v1/capabilities`)).status).toBe(200)
    } finally {
      await api.stop()
    }
  })
})
